//! Several addresses per server, and roaming between them (ADR 0014).
//!
//! [`RoamingAddresses`] keeps the user's ordered list of a server's addresses and picks one by a staggered race of identity probes
//! (Happy Eyeballs, RFC 8305, one level up): the first address whose [`IdentityProbe`] passes wins. It races again when the address in
//! use fails, when the network changes, and every few minutes while a better address is down; it remembers the winner per network.
//!
//! Credentials go only to an address that passed its identity check in the race that selected it: probes carry none, and nothing is
//! served while a race after a failure or a network change runs. An address that needs the user's confirmation for plain HTTP (ADR
//! 0012) and lacks it is never probed. Addresses never appear in logs, errors or Debug output: the log names an address by its position
//! in the user's list.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use current_model::{
    Connectivity, NetworkFacts, NetworkKey, Reachability, ServerAddress, ServerRef,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, Sleep};
use url::Url;

use crate::address::{AddressOutcome, AddressProvider, ProbeAddress, Selected};
use crate::client::{ClientConfig, ServerClient};
use crate::log::RequestLog;
use crate::scope::transport;

/// The tokio runtime a [`RoamingAddresses`] runs its races on. Re-exported so that callers need no tokio features of their own.
pub use tokio::runtime::Handle as RuntimeHandle;

/// How many networks' winners a server remembers (ADR 0014).
const CACHED_NETWORKS: usize = 16;

/// What an identity probe found at an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityCheck {
    /// The expected server answered.
    Passed,
    /// Another server answered: the address is skipped until the user edits it.
    DifferentServer,
    /// No usable answer. Why is in the request log.
    Failed,
}

/// Checks that an address leads to the expected server; implemented by the service crates (Jellyfin compares its server id).
pub trait IdentityProbe: Send + Sync {
    /// Probes the address `client` is bound to. `client` carries no credentials and logs to the shared request log.
    fn check<'a>(
        &'a self,
        client: &'a ServerClient,
    ) -> Pin<Box<dyn Future<Output = IdentityCheck> + Send + 'a>>;
}

/// Timings of the race and of racing again. The defaults are ADR 0014's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoamingConfig {
    /// Between the starts of two probes (RFC 8305's default).
    pub stagger: Duration,
    /// How long one probe may take.
    pub probe_timeout: Duration,
    /// How often better addresses are probed again while a worse one is in use and the server is used.
    pub upgrade_interval: Duration,
    /// The first wait before racing again after a failed race; it doubles up to `backoff_max`.
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for RoamingConfig {
    fn default() -> Self {
        Self {
            stagger: Duration::from_millis(250),
            probe_timeout: Duration::from_secs(3),
            upgrade_interval: Duration::from_secs(5 * 60),
            backoff_min: Duration::from_secs(5),
            backoff_max: Duration::from_secs(5 * 60),
        }
    }
}

/// What the server's settings screen and the capabilities state show about its addresses. No address, only positions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoamingStatus {
    pub reachability: Reachability,
    /// The 1-based position of the address in use.
    pub position: Option<u8>,
    /// Positions of addresses that answered as another server, in the user's order.
    pub different_server: Vec<u8>,
}

/// Why a list of addresses cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoamingError {
    /// The address at this 1-based position is not an http(s) URL.
    #[error("address {0} is not a valid address")]
    InvalidAddress(u8),
    /// More addresses than positions in the log (255).
    #[error("a server can have at most 255 addresses")]
    TooManyAddresses,
    /// A probe client could not be built (the TLS backend did not start).
    #[error("the secure connection could not be set up")]
    Tls,
}

/// Where the provider is. Mirrors ADR 0014's states.
#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Nothing tried yet.
    Idle,
    /// A race runs; `serving` is the address still in use meanwhile (only during an upgrade check).
    Racing {
        serving: Option<Selected>,
    },
    Selected(Selected),
    /// The last race failed, or the device has no network.
    Unreachable,
}

/// The state with the number of the race that last changed it. Callers bump the number when they start a race; a race's result is
/// published only if no newer race started meanwhile.
#[derive(Debug)]
struct Published {
    epoch: u64,
    state: State,
}

/// Which race the driver should run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RaceKind {
    /// Every usable address, the network's cached winner first.
    Full,
    /// Only the addresses before the one in use, which keeps serving.
    Upgrade,
}

#[derive(Debug)]
enum Command {
    /// Run a race for this epoch, unless a newer one started.
    Race { epoch: u64, kind: RaceKind },
    /// The network changed; `epoch` is the race it started, if any.
    Network {
        facts: NetworkFacts,
        epoch: Option<u64>,
    },
    /// The server is in use again after an idle stretch: the upgrade check resumes.
    Used,
}

/// One address of the list.
struct Candidate {
    url: Url,
    position: u8,
    /// `None` for an address that is never used: plain HTTP that needs a confirmation the user did not give.
    client: Option<Arc<ServerClient>>,
}

/// State shared between the provider's handle and its driver task.
struct Shared {
    state: watch::Sender<Published>,
    status: watch::Sender<RoamingStatus>,
    /// A request reported since the last upgrade check.
    used: AtomicBool,
    /// The upgrade check stopped because the server was idle.
    upgrade_paused: AtomicBool,
    /// The facts last reported, to ignore repeats.
    facts: Mutex<Option<NetworkFacts>>,
}

impl Shared {
    /// Changes the state if `change` says so, then the status.
    fn update(&self, change: impl FnOnce(&mut Published) -> bool) -> bool {
        let changed = self.state.send_if_modified(change);
        if changed {
            let state = self.state.borrow().state.clone();
            self.status.send_if_modified(|status| {
                let reachability = match &state {
                    State::Idle => Reachability::Unknown,
                    State::Racing { serving: None } => status.reachability,
                    State::Racing { serving: Some(_) } | State::Selected(_) => {
                        Reachability::Reachable
                    }
                    State::Unreachable => Reachability::Unreachable,
                };
                let position = match &state {
                    State::Racing { serving: Some(s) } | State::Selected(s) => s.position(),
                    _ => None,
                };
                let changed = status.reachability != reachability || status.position != position;
                status.reachability = reachability;
                status.position = position;
                changed
            });
        }
        changed
    }

    /// Starts a race from the caller's side: from now on nothing is served (unless `serving` is kept) and a newer race result wins.
    /// Returns the race's epoch.
    fn begin_race(&self, serving: Option<Selected>) -> u64 {
        let mut epoch = 0;
        self.update(|published| {
            published.epoch += 1;
            epoch = published.epoch;
            published.state = State::Racing { serving };
            true
        });
        epoch
    }

    fn serving(&self) -> Option<Selected> {
        match &self.state.borrow().state {
            State::Racing { serving: Some(s) } | State::Selected(s) => Some(s.clone()),
            _ => None,
        }
    }
}

/// A server with several addresses, chosen by racing identity probes (ADR 0014). Build it with [`RoamingAddresses::new`] and hand it
/// to a [`ServerClient`] as its [`AddressProvider`].
pub struct RoamingAddresses {
    server: ServerRef,
    shared: Arc<Shared>,
    commands: mpsc::UnboundedSender<Command>,
}

impl std::fmt::Debug for RoamingAddresses {
    /// The server and the state, with positions only: no address.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoamingAddresses")
            .field("server", &self.server)
            .field("state", &self.shared.state.borrow().state)
            .finish_non_exhaustive()
    }
}

impl RoamingAddresses {
    /// A provider for `server` over `addresses` in the user's order. One probe client per usable address is built now and reused
    /// (building one sets up TLS, which takes noticeable time: make providers once per server, not per request); probes are logged
    /// to `log`. Races run on `runtime`; nothing runs until the first request or [`RoamingAddresses::retry_now`].
    ///
    /// # Errors
    /// [`RoamingError`] for an address that is not an http(s) URL, more than 255 addresses, or a TLS backend that does not start.
    pub fn new(
        server: ServerRef,
        addresses: &[ServerAddress],
        probe: Arc<dyn IdentityProbe>,
        log: Arc<RequestLog>,
        config: RoamingConfig,
        runtime: &RuntimeHandle,
    ) -> Result<Arc<Self>, RoamingError> {
        if addresses.len() > usize::from(u8::MAX) {
            return Err(RoamingError::TooManyAddresses);
        }
        let probe_config = ClientConfig {
            timeout: config.probe_timeout,
            connect_timeout: config.probe_timeout,
        };
        let mut candidates = Vec::with_capacity(addresses.len());
        for (index, address) in addresses.iter().enumerate() {
            // At most 255 addresses, checked above.
            let position = u8::try_from(index + 1).map_err(|_| RoamingError::TooManyAddresses)?;
            let url = Url::parse(&address.url)
                .ok()
                .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
                .ok_or(RoamingError::InvalidAddress(position))?;
            let usable = !transport(&url).needs_confirmation() || address.plain_http_confirmed;
            let client = if usable {
                let selected = Selected::new(url.clone(), Some(position), 0);
                let client = ServerClient::new(
                    server.clone(),
                    Arc::new(ProbeAddress(selected)),
                    None,
                    log.clone(),
                    &probe_config,
                )
                .map_err(|_| RoamingError::Tls)?;
                Some(Arc::new(client))
            } else {
                None
            };
            candidates.push(Candidate {
                url,
                position,
                client,
            });
        }
        let shared = Arc::new(Shared {
            state: watch::Sender::new(Published {
                epoch: 0,
                state: State::Idle,
            }),
            status: watch::Sender::new(RoamingStatus::default()),
            used: AtomicBool::new(false),
            upgrade_paused: AtomicBool::new(false),
            facts: Mutex::new(None),
        });
        let (commands, receiver) = mpsc::unbounded_channel();
        let driver = Driver {
            shared: shared.clone(),
            candidates,
            probe,
            config,
            commands: receiver,
            different: Vec::new(),
            network: None,
            no_network: false,
            cache: WinnerCache::default(),
            backoff: None,
            race: None,
            timer: None,
        };
        runtime.spawn(driver.run());
        Ok(Arc::new(Self {
            server,
            shared,
            commands,
        }))
    }

    /// The addresses' state now, and every change after.
    pub fn status(&self) -> watch::Receiver<RoamingStatus> {
        self.shared.status.subscribe()
    }

    /// The platform reported the network (on start and on every change). A real change stops serving the address in use until it
    /// passes its identity check again on the new network, and races at once with the new network's cached winner first. Before the
    /// first request it only records the facts.
    pub fn network_changed(&self, facts: &NetworkFacts) {
        {
            let mut last = self
                .shared
                .facts
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if last.as_ref() == Some(facts) {
                return;
            }
            *last = Some(facts.clone());
        }
        let idle = matches!(self.shared.state.borrow().state, State::Idle);
        let epoch = if idle {
            None
        } else if facts.connectivity == Connectivity::None {
            // No race: nothing can answer. The driver cancels what runs and waits for the next change.
            self.shared.update(|published| {
                published.epoch += 1;
                published.state = State::Unreachable;
                true
            });
            None
        } else {
            Some(self.shared.begin_race(None))
        };
        let _ = self.commands.send(Command::Network {
            facts: facts.clone(),
            epoch,
        });
    }

    /// Races now if no address is selected (nothing tried yet, or the last race failed): for the app returning to the foreground.
    pub fn retry_now(&self) {
        if matches!(
            self.shared.state.borrow().state,
            State::Idle | State::Unreachable
        ) {
            self.start_full_race();
        }
    }

    fn start_full_race(&self) {
        let epoch = self.shared.begin_race(None);
        let _ = self.commands.send(Command::Race {
            epoch,
            kind: RaceKind::Full,
        });
    }

    /// Starts the first race if nothing was tried yet.
    fn start_if_idle(&self) {
        let mut epoch = None;
        self.shared.update(|published| {
            if published.state != State::Idle {
                return false;
            }
            published.epoch += 1;
            epoch = Some(published.epoch);
            published.state = State::Racing { serving: None };
            true
        });
        if let Some(epoch) = epoch {
            let _ = self.commands.send(Command::Race {
                epoch,
                kind: RaceKind::Full,
            });
        }
    }
}

impl AddressProvider for RoamingAddresses {
    fn current(&self) -> Option<Selected> {
        let serving = self.shared.serving();
        if serving.is_none() {
            self.start_if_idle();
        }
        serving
    }

    fn wait(
        &self,
        deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>> {
        Box::pin(async move {
            self.start_if_idle();
            let mut state = self.shared.state.subscribe();
            loop {
                match &state.borrow_and_update().state {
                    State::Racing { serving: Some(s) } | State::Selected(s) => {
                        return Some(s.clone());
                    }
                    State::Unreachable => return None,
                    State::Idle | State::Racing { serving: None } => {}
                }
                match tokio::time::timeout_at(deadline, state.changed()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) | Err(_) => return None,
                }
            }
        })
    }

    fn report(&self, selected: &Selected, outcome: AddressOutcome) {
        self.shared.used.store(true, Ordering::Relaxed);
        if outcome == AddressOutcome::Ok {
            if self.shared.upgrade_paused.swap(false, Ordering::Relaxed) {
                let _ = self.commands.send(Command::Used);
            }
            return;
        }
        let mut epoch = None;
        self.shared.update(|published| {
            let failed = match &published.state {
                State::Racing { serving: Some(s) } | State::Selected(s) => {
                    s.generation == selected.generation && s.url() == selected.url()
                }
                _ => false,
            };
            if failed {
                published.epoch += 1;
                epoch = Some(published.epoch);
                published.state = State::Racing { serving: None };
            }
            failed
        });
        if let Some(epoch) = epoch {
            let _ = self.commands.send(Command::Race {
                epoch,
                kind: RaceKind::Full,
            });
        }
    }
}

/// What a race reported.
enum RaceEvent {
    /// The candidate at this index passed its identity check.
    Won(usize),
    /// The candidate at this index is another server.
    Different(usize),
    /// Every candidate failed.
    Lost,
}

/// A running race: candidates still to start, probes running, and when the next may start.
struct Race {
    epoch: u64,
    kind: RaceKind,
    /// Indexes into the candidates, in the order they start.
    queue: std::collections::VecDeque<usize>,
    running: JoinSet<(usize, IdentityCheck)>,
    next_start: Pin<Box<Sleep>>,
    stagger: Duration,
    probe_timeout: Duration,
    clients: Vec<(usize, Arc<ServerClient>)>,
    probe: Arc<dyn IdentityProbe>,
}

impl Race {
    fn start_next(&mut self) {
        let Some(index) = self.queue.pop_front() else {
            return;
        };
        let Some(client) = self
            .clients
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, client)| client.clone())
        else {
            return;
        };
        let probe = self.probe.clone();
        let timeout = self.probe_timeout;
        self.running.spawn(async move {
            let check = match tokio::time::timeout(timeout, probe.check(&client)).await {
                Ok(check) => check,
                Err(_) => IdentityCheck::Failed,
            };
            (index, check)
        });
        self.next_start
            .as_mut()
            .reset(Instant::now() + self.stagger);
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<RaceEvent> {
        loop {
            if !self.queue.is_empty()
                && (self.running.is_empty() || self.next_start.as_mut().poll(cx).is_ready())
            {
                self.start_next();
                continue;
            }
            match self.running.poll_join_next(cx) {
                Poll::Ready(Some(Ok((index, IdentityCheck::Passed)))) => {
                    return Poll::Ready(RaceEvent::Won(index));
                }
                Poll::Ready(Some(Ok((index, IdentityCheck::DifferentServer)))) => {
                    return Poll::Ready(RaceEvent::Different(index));
                }
                // A failed or panicked probe: the next starts at once once none runs.
                Poll::Ready(Some(Ok((_, IdentityCheck::Failed)) | Err(_))) => {}
                Poll::Ready(None) if self.queue.is_empty() => {
                    return Poll::Ready(RaceEvent::Lost);
                }
                Poll::Ready(None) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Why the driver's timer is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimerKind {
    /// Race again after a failed race.
    Retry,
    /// Probe the better addresses again.
    Upgrade,
}

/// What woke the driver.
enum Event {
    Command(Command),
    Race(RaceEvent),
    Timer(TimerKind),
    /// The provider was dropped.
    Closed,
}

/// The task that runs a provider's races and timers. Callers change the state for what they start; the driver publishes results.
struct Driver {
    shared: Arc<Shared>,
    candidates: Vec<Candidate>,
    probe: Arc<dyn IdentityProbe>,
    config: RoamingConfig,
    commands: mpsc::UnboundedReceiver<Command>,
    /// Indexes of candidates that answered as another server.
    different: Vec<usize>,
    network: Option<NetworkKey>,
    /// The platform reports no network: no races until it changes.
    no_network: bool,
    cache: WinnerCache,
    /// The wait before the next retry; `None` until a race fails.
    backoff: Option<Duration>,
    race: Option<Race>,
    timer: Option<(Pin<Box<Sleep>>, TimerKind)>,
}

impl Driver {
    async fn run(mut self) {
        loop {
            let event = poll_fn(|cx| self.poll_event(cx)).await;
            match event {
                Event::Closed => return,
                Event::Command(command) => self.on_command(command),
                Event::Race(event) => self.on_race(event),
                Event::Timer(kind) => self.on_timer(kind),
            }
        }
    }

    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Event> {
        match self.commands.poll_recv(cx) {
            Poll::Ready(Some(command)) => return Poll::Ready(Event::Command(command)),
            Poll::Ready(None) => return Poll::Ready(Event::Closed),
            Poll::Pending => {}
        }
        if let Some(race) = &mut self.race
            && let Poll::Ready(event) = race.poll(cx)
        {
            return Poll::Ready(Event::Race(event));
        }
        if let Some((sleep, kind)) = &mut self.timer
            && sleep.as_mut().poll(cx).is_ready()
        {
            let kind = *kind;
            self.timer = None;
            return Poll::Ready(Event::Timer(kind));
        }
        Poll::Pending
    }

    fn epoch(&self) -> u64 {
        self.shared.state.borrow().epoch
    }

    fn on_command(&mut self, command: Command) {
        match command {
            Command::Race { epoch, kind } => {
                if epoch == self.epoch() {
                    self.start_race(epoch, kind);
                }
            }
            Command::Network { facts, epoch } => {
                self.network = facts.network_key;
                self.no_network = facts.connectivity == Connectivity::None;
                self.backoff = None;
                self.race = None;
                self.timer = None;
                if let Some(epoch) = epoch
                    && epoch == self.epoch()
                {
                    self.start_race(epoch, RaceKind::Full);
                }
            }
            Command::Used => {
                if self.timer.is_none() && self.race.is_none() {
                    self.arm_upgrade();
                }
            }
        }
    }

    /// Indexes of the candidates a race may probe, in the user's order.
    fn usable(&self) -> impl Iterator<Item = usize> + '_ {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                candidate.client.is_some() && !self.different.contains(index)
            })
            .map(|(index, _)| index)
    }

    fn start_race(&mut self, epoch: u64, kind: RaceKind) {
        self.timer = None;
        let queue: std::collections::VecDeque<usize> = match kind {
            RaceKind::Full => {
                let cached = self.network.clone().and_then(|key| self.cache.get(&key));
                let cached = cached.filter(|index| self.usable().any(|i| i == *index));
                cached
                    .into_iter()
                    .chain(self.usable().filter(|index| Some(*index) != cached))
                    .collect()
            }
            RaceKind::Upgrade => {
                let current = self
                    .shared
                    .serving()
                    .and_then(|s| s.position())
                    .map_or(0, usize::from);
                self.usable().filter(|index| index + 1 < current).collect()
            }
        };
        let clients = queue
            .iter()
            .filter_map(|index| {
                self.candidates[*index]
                    .client
                    .clone()
                    .map(|client| (*index, client))
            })
            .collect();
        self.race = Some(Race {
            epoch,
            kind,
            queue,
            running: JoinSet::new(),
            next_start: Box::pin(tokio::time::sleep(Duration::ZERO)),
            stagger: self.config.stagger,
            probe_timeout: self.config.probe_timeout,
            clients,
            probe: self.probe.clone(),
        });
    }

    fn on_race(&mut self, event: RaceEvent) {
        let Some(race) = &self.race else {
            return;
        };
        let (epoch, kind) = (race.epoch, race.kind);
        match event {
            RaceEvent::Different(index) => {
                if !self.different.contains(&index) {
                    self.different.push(index);
                    self.different.sort_unstable();
                    let positions: Vec<u8> = self
                        .different
                        .iter()
                        .map(|index| self.candidates[*index].position)
                        .collect();
                    self.shared.status.send_if_modified(|status| {
                        let changed = status.different_server != positions;
                        status.different_server = positions;
                        changed
                    });
                }
            }
            RaceEvent::Won(index) => {
                self.race = None;
                self.backoff = None;
                let candidate = &self.candidates[index];
                let selected =
                    Selected::new(candidate.url.clone(), Some(candidate.position), epoch);
                let published = self.shared.update(|published| {
                    if published.epoch != epoch {
                        return false;
                    }
                    published.state = State::Selected(selected);
                    true
                });
                if published {
                    if let Some(key) = self.network.clone() {
                        self.cache.insert(key, index);
                    }
                    self.shared.used.store(false, Ordering::Relaxed);
                    self.arm_upgrade();
                }
            }
            RaceEvent::Lost => {
                self.race = None;
                match kind {
                    RaceKind::Upgrade => {
                        // The address in use keeps serving.
                        let published = self.shared.update(|published| {
                            if published.epoch != epoch {
                                return false;
                            }
                            match &published.state {
                                State::Racing { serving: Some(s) } => {
                                    published.state = State::Selected(s.clone());
                                    true
                                }
                                _ => false,
                            }
                        });
                        if published {
                            self.arm_upgrade();
                        }
                    }
                    RaceKind::Full => {
                        let published = self.shared.update(|published| {
                            if published.epoch != epoch {
                                return false;
                            }
                            published.state = State::Unreachable;
                            true
                        });
                        if published && !self.no_network {
                            let wait = self.backoff.unwrap_or(self.config.backoff_min);
                            self.backoff = Some((wait * 2).min(self.config.backoff_max));
                            self.timer =
                                Some((Box::pin(tokio::time::sleep(wait)), TimerKind::Retry));
                        }
                    }
                }
            }
        }
    }

    fn on_timer(&mut self, kind: TimerKind) {
        match kind {
            TimerKind::Retry => {
                if self.shared.state.borrow().state == State::Unreachable {
                    let epoch = self.shared.begin_race(None);
                    self.start_race(epoch, RaceKind::Full);
                }
            }
            TimerKind::Upgrade => {
                let state = self.shared.state.borrow().state.clone();
                let State::Selected(selected) = state else {
                    return;
                };
                if self.shared.used.swap(false, Ordering::Relaxed) {
                    let epoch = self.shared.begin_race(Some(selected));
                    self.start_race(epoch, RaceKind::Upgrade);
                } else {
                    // Idle: no polling until the next request.
                    self.shared.upgrade_paused.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// Sets the upgrade timer when the address in use has a better usable one before it.
    fn arm_upgrade(&mut self) {
        let state = self.shared.state.borrow().state.clone();
        let State::Selected(selected) = state else {
            return;
        };
        let current = selected.position().map_or(0, usize::from);
        if self.usable().any(|index| index + 1 < current) {
            self.timer = Some((
                Box::pin(tokio::time::sleep(self.config.upgrade_interval)),
                TimerKind::Upgrade,
            ));
        }
    }
}

/// The winning address per network, in memory, for at most [`CACHED_NETWORKS`] networks; the least recently used goes first.
#[derive(Debug, Default)]
struct WinnerCache {
    /// Least recently used first.
    entries: Vec<(NetworkKey, usize)>,
}

impl WinnerCache {
    /// The winner on `key`, marked as recently used.
    fn get(&mut self, key: &NetworkKey) -> Option<usize> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(at);
        let index = entry.1;
        self.entries.push(entry);
        Some(index)
    }

    fn insert(&mut self, key: NetworkKey, index: usize) {
        self.entries.retain(|(k, _)| *k != key);
        if self.entries.len() >= CACHED_NETWORKS {
            self.entries.remove(0);
        }
        self.entries.push((key, index));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use current_model::{InterfaceKind, ServerKey, ServiceType};

    use super::*;

    /// What the scripted probe does at an address.
    #[derive(Clone, Copy, Debug)]
    enum Probe {
        Pass(u64),
        Different(u64),
        Fail(u64),
        /// Never answers: the probe timeout ends it.
        Hang,
    }

    /// An identity probe that follows a script per position and records when each probe started.
    struct Script {
        started: Instant,
        behaviour: Mutex<HashMap<u8, Probe>>,
        calls: Mutex<Vec<(u8, Duration)>>,
    }

    impl Script {
        fn new(behaviour: &[Probe]) -> Arc<Self> {
            let behaviour = behaviour
                .iter()
                .enumerate()
                .map(|(index, probe)| (u8::try_from(index + 1).unwrap(), *probe))
                .collect();
            Arc::new(Self {
                started: Instant::now(),
                behaviour: Mutex::new(behaviour),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn set(&self, position: u8, probe: Probe) {
            self.behaviour.lock().unwrap().insert(position, probe);
        }

        /// The positions probed, in order.
        fn probed(&self) -> Vec<u8> {
            self.calls.lock().unwrap().iter().map(|(p, _)| *p).collect()
        }

        /// When each probe started, in milliseconds since the script was made.
        fn starts(&self) -> Vec<(u8, u128)> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(p, at)| (*p, at.as_millis()))
                .collect()
        }
    }

    impl IdentityProbe for Script {
        fn check<'a>(
            &'a self,
            client: &'a ServerClient,
        ) -> Pin<Box<dyn Future<Output = IdentityCheck> + Send + 'a>> {
            Box::pin(async move {
                let position = client.current_address().unwrap().position().unwrap();
                self.calls
                    .lock()
                    .unwrap()
                    .push((position, self.started.elapsed()));
                let probe = self.behaviour.lock().unwrap()[&position];
                let ms = Duration::from_millis;
                match probe {
                    Probe::Pass(delay) => {
                        tokio::time::sleep(ms(delay)).await;
                        IdentityCheck::Passed
                    }
                    Probe::Different(delay) => {
                        tokio::time::sleep(ms(delay)).await;
                        IdentityCheck::DifferentServer
                    }
                    Probe::Fail(delay) => {
                        tokio::time::sleep(ms(delay)).await;
                        IdentityCheck::Failed
                    }
                    Probe::Hang => std::future::pending().await,
                }
            })
        }
    }

    fn server() -> ServerRef {
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
    }

    fn local(count: u16) -> Vec<ServerAddress> {
        (1..=count)
            .map(|port| ServerAddress {
                url: format!("http://127.0.0.1:{port}"),
                plain_http_confirmed: false,
            })
            .collect()
    }

    fn roaming_with(
        addresses: &[ServerAddress],
        script: &Arc<Script>,
        config: RoamingConfig,
    ) -> Arc<RoamingAddresses> {
        RoamingAddresses::new(
            server(),
            addresses,
            script.clone(),
            Arc::new(RequestLog::default()),
            config,
            &RuntimeHandle::current(),
        )
        .unwrap()
    }

    fn roaming(script: &Arc<Script>) -> Arc<RoamingAddresses> {
        let count = script.behaviour.lock().unwrap().len();
        roaming_with(
            &local(u16::try_from(count).unwrap()),
            script,
            RoamingConfig::default(),
        )
    }

    /// The address selected once the race ends, with how long it took.
    async fn selected(roaming: &RoamingAddresses) -> (Option<Selected>, u128) {
        let start = Instant::now();
        let selected = roaming.wait(start + Duration::from_secs(600)).await;
        (selected, start.elapsed().as_millis())
    }

    async fn position(roaming: &RoamingAddresses) -> Option<u8> {
        selected(roaming).await.0.and_then(|s| s.position())
    }

    fn network(key: &str) -> NetworkFacts {
        NetworkFacts {
            connectivity: Connectivity::Full,
            interface: InterfaceKind::WiFi,
            network_key: Some(NetworkKey::new(key)),
            ..NetworkFacts::default()
        }
    }

    /// Lets the driver run everything that is due now.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_address_that_answers_wins() {
        let script = Script::new(&[Probe::Pass(5), Probe::Pass(5)]);
        let roaming = roaming(&script);
        let (winner, took) = selected(&roaming).await;
        assert_eq!(winner.and_then(|s| s.position()), Some(1));
        assert_eq!(took, 5);
        assert_eq!(script.probed(), [1]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_address_starts_the_next_at_once() {
        let script = Script::new(&[Probe::Fail(1), Probe::Pass(5)]);
        let roaming = roaming(&script);
        let (winner, took) = selected(&roaming).await;
        assert_eq!(winner.and_then(|s| s.position()), Some(2));
        assert_eq!(script.starts(), [(1, 0), (2, 1)]);
        assert_eq!(took, 6);
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_address_costs_one_stagger() {
        let script = Script::new(&[Probe::Hang, Probe::Pass(5)]);
        let roaming = roaming(&script);
        let (winner, took) = selected(&roaming).await;
        assert_eq!(winner.and_then(|s| s.position()), Some(2));
        assert_eq!(script.starts(), [(1, 0), (2, 250)]);
        assert_eq!(took, 255);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_first_address_within_the_stagger_wins() {
        let script = Script::new(&[Probe::Pass(150), Probe::Pass(5)]);
        let roaming = roaming(&script);
        let (winner, took) = selected(&roaming).await;
        assert_eq!(winner.and_then(|s| s.position()), Some(1));
        assert_eq!(took, 150);
        assert_eq!(script.probed(), [1]);
    }

    #[tokio::test(start_paused = true)]
    async fn another_server_is_skipped_and_marked() {
        let script = Script::new(&[Probe::Different(5), Probe::Pass(5)]);
        let roaming = roaming(&script);
        let (winner, _) = selected(&roaming).await;
        let winner = winner.unwrap();
        assert_eq!(winner.position(), Some(2));
        assert_eq!(roaming.status().borrow().different_server, [1]);

        // The next race does not probe it again.
        roaming.report(&winner, AddressOutcome::Unreachable);
        assert_eq!(position(&roaming).await, Some(2));
        assert_eq!(script.probed(), [1, 2, 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn all_down_ends_unreachable_after_the_probe_timeout() {
        let script = Script::new(&[Probe::Hang, Probe::Fail(1), Probe::Hang]);
        let roaming = roaming(&script);
        let (winner, took) = selected(&roaming).await;
        assert_eq!(winner, None);
        // Address 2 failing does not start address 3 early: address 1 still runs.
        assert_eq!(script.starts(), [(1, 0), (2, 250), (3, 500)]);
        assert_eq!(took, 3500);
        assert_eq!(
            roaming.status().borrow().reachability,
            Reachability::Unreachable
        );
        // Requests fail fast while unreachable.
        assert_eq!(selected(&roaming).await, (None, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn plain_http_without_confirmation_is_never_probed() {
        let script = Script::new(&[Probe::Pass(1), Probe::Pass(1)]);
        let mut addresses = local(2);
        addresses[0] = ServerAddress {
            url: "http://media.example.com".into(),
            plain_http_confirmed: false,
        };
        let roaming = roaming_with(&addresses, &script, RoamingConfig::default());
        assert_eq!(position(&roaming).await, Some(2));
        assert_eq!(script.probed(), [2]);

        addresses[0].plain_http_confirmed = true;
        let script = Script::new(&[Probe::Pass(1), Probe::Pass(1)]);
        let roaming = roaming_with(&addresses, &script, RoamingConfig::default());
        assert_eq!(position(&roaming).await, Some(1));
    }

    #[test]
    fn invalid_lists_are_refused() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let script = Script::new(&[]);
        let new = |addresses: &[ServerAddress]| {
            RoamingAddresses::new(
                server(),
                addresses,
                script.clone(),
                Arc::new(RequestLog::default()),
                RoamingConfig::default(),
                runtime.handle(),
            )
            .map(|_| ())
        };
        let mut addresses = local(2);
        addresses[1].url = "mailto:someone@example.com".into();
        assert_eq!(new(&addresses), Err(RoamingError::InvalidAddress(2)));
        addresses[1].url = "not a url".into();
        assert_eq!(new(&addresses), Err(RoamingError::InvalidAddress(2)));
        let many: Vec<ServerAddress> = (0..256).map(|_| local(1)[0].clone()).collect();
        assert_eq!(new(&many), Err(RoamingError::TooManyAddresses));
    }

    #[tokio::test(start_paused = true)]
    async fn a_network_s_winner_goes_first_on_that_network_only() {
        let script = Script::new(&[Probe::Fail(1), Probe::Pass(1)]);
        let roaming = roaming(&script);
        roaming.network_changed(&network("home"));
        assert_eq!(position(&roaming).await, Some(2));

        script.set(1, Probe::Pass(1));
        roaming.network_changed(&network("away"));
        assert_eq!(position(&roaming).await, Some(1));

        // Back home the cached winner goes first, and its identity is still checked.
        script.calls.lock().unwrap().clear();
        roaming.network_changed(&network("home"));
        assert_eq!(position(&roaming).await, Some(2));
        assert_eq!(script.probed(), [2]);
    }

    #[test]
    fn the_cache_keeps_the_last_16_networks() {
        let mut cache = WinnerCache::default();
        for n in 0..17 {
            cache.insert(NetworkKey::new(format!("n{n}")), n);
        }
        assert_eq!(cache.get(&NetworkKey::new("n0")), None);
        assert_eq!(cache.get(&NetworkKey::new("n1")), Some(1));
        // n1 was used last, so n2 goes next.
        cache.insert(NetworkKey::new("n17"), 17);
        assert_eq!(cache.get(&NetworkKey::new("n2")), None);
        assert_eq!(cache.get(&NetworkKey::new("n1")), Some(1));
        cache.insert(NetworkKey::new("n1"), 5);
        assert_eq!(cache.get(&NetworkKey::new("n1")), Some(5));
        assert_eq!(cache.entries.len(), CACHED_NETWORKS);
    }

    #[tokio::test(start_paused = true)]
    async fn many_failures_of_one_address_start_one_race() {
        let script = Script::new(&[Probe::Pass(5), Probe::Pass(5)]);
        let roaming = roaming(&script);
        let first = selected(&roaming).await.0.unwrap();
        for _ in 0..5 {
            roaming.report(&first, AddressOutcome::Timeout);
        }
        assert_eq!(roaming.current(), None, "a failed address stops serving");
        let second = selected(&roaming).await.0.unwrap();
        assert_eq!(script.probed(), [1, 1]);

        // A late report about the earlier choice, and a success, start nothing.
        roaming.report(&first, AddressOutcome::Unreachable);
        roaming.report(&second, AddressOutcome::Ok);
        settle().await;
        assert_eq!(roaming.current(), Some(second));
        assert_eq!(script.probed(), [1, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_network_change_races_again_before_serving() {
        let script = Script::new(&[Probe::Pass(5)]);
        let roaming = roaming(&script);
        assert_eq!(position(&roaming).await, Some(1));
        roaming.network_changed(&network("away"));
        assert_eq!(roaming.current(), None);
        assert_eq!(
            roaming.status().borrow().reachability,
            Reachability::Reachable
        );
        assert_eq!(position(&roaming).await, Some(1));
        assert_eq!(script.probed(), [1, 1]);

        // The same facts again are not a change.
        roaming.network_changed(&network("away"));
        assert!(roaming.current().is_some());
        settle().await;
        assert_eq!(script.probed(), [1, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn facts_before_the_first_request_start_nothing() {
        let script = Script::new(&[Probe::Pass(5)]);
        let roaming = roaming(&script);
        roaming.network_changed(&network("home"));
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(script.probed().is_empty());
        assert_eq!(
            roaming.status().borrow().reachability,
            Reachability::Unknown
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_upgrade_check_returns_to_a_better_address() {
        let script = Script::new(&[Probe::Fail(1), Probe::Pass(1)]);
        let roaming = roaming(&script);
        let second = selected(&roaming).await.0.unwrap();
        assert_eq!(second.position(), Some(2));
        script.set(1, Probe::Pass(2000));

        // In use: a request every minute.
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_secs(60)).await;
            roaming.report(&second, AddressOutcome::Ok);
        }
        assert_eq!(script.probed(), [1, 2]);
        tokio::time::sleep(Duration::from_secs(61)).await;
        // The check runs on the better address only, and the worse one keeps serving meanwhile.
        assert_eq!(script.probed(), [1, 2, 1]);
        assert_eq!(roaming.current().and_then(|s| s.position()), Some(2));
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(roaming.current().and_then(|s| s.position()), Some(1));
        assert_eq!(roaming.status().borrow().position, Some(1));
    }

    #[tokio::test(start_paused = true)]
    async fn the_upgrade_check_stops_while_idle() {
        let script = Script::new(&[Probe::Fail(1), Probe::Pass(1)]);
        let roaming = roaming(&script);
        let second = selected(&roaming).await.0.unwrap();
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(script.probed(), [1, 2]);

        // In use again: the check resumes one interval later.
        roaming.report(&second, AddressOutcome::Ok);
        tokio::time::sleep(Duration::from_secs(4 * 60)).await;
        roaming.report(&second, AddressOutcome::Ok);
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(script.probed(), [1, 2, 1]);
        assert_eq!(roaming.current().and_then(|s| s.position()), Some(2));
    }

    #[tokio::test(start_paused = true)]
    async fn failed_races_back_off_and_a_network_change_resets() {
        let script = Script::new(&[Probe::Fail(0)]);
        let config = RoamingConfig {
            backoff_max: Duration::from_secs(40),
            ..RoamingConfig::default()
        };
        let roaming = roaming_with(&local(1), &script, config);
        assert_eq!(selected(&roaming).await.0, None);
        tokio::time::sleep(Duration::from_secs(200)).await;
        let seconds: Vec<u128> = script.starts().iter().map(|(_, ms)| ms / 1000).collect();
        // 5, 10, 20, then the 40 s cap.
        assert_eq!(seconds, [0, 5, 15, 35, 75, 115, 155, 195]);

        script.calls.lock().unwrap().clear();
        let changed = Instant::now();
        roaming.network_changed(&network("away"));
        tokio::time::sleep(Duration::from_secs(16)).await;
        let since: Vec<u128> = script
            .starts()
            .iter()
            .map(|(_, ms)| (ms - (changed - script.started).as_millis()) / 1000)
            .collect();
        assert_eq!(since, [0, 5, 15]);
    }

    #[tokio::test(start_paused = true)]
    async fn no_network_means_no_races_until_it_changes() {
        let script = Script::new(&[Probe::Pass(1)]);
        let roaming = roaming(&script);
        assert_eq!(position(&roaming).await, Some(1));
        roaming.network_changed(&NetworkFacts {
            connectivity: Connectivity::None,
            ..NetworkFacts::default()
        });
        assert_eq!(roaming.current(), None);
        assert_eq!(
            roaming.status().borrow().reachability,
            Reachability::Unreachable
        );
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(script.probed(), [1]);

        roaming.network_changed(&network("home"));
        assert_eq!(position(&roaming).await, Some(1));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_now_races_when_unreachable() {
        let script = Script::new(&[Probe::Fail(0)]);
        let roaming = roaming(&script);
        assert_eq!(selected(&roaming).await.0, None);
        script.set(1, Probe::Pass(1));
        roaming.retry_now();
        assert_eq!(position(&roaming).await, Some(1));
        assert_eq!(script.probed(), [1, 1]);
        // Selected: nothing to do.
        roaming.retry_now();
        settle().await;
        assert_eq!(script.probed(), [1, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn states_map_to_reachability() {
        let script = Script::new(&[Probe::Pass(10)]);
        let roaming = roaming(&script);
        let status = roaming.status();
        assert_eq!(status.borrow().reachability, Reachability::Unknown);
        // The first race: still unknown.
        assert_eq!(roaming.current(), None);
        assert_eq!(status.borrow().reachability, Reachability::Unknown);
        let first = selected(&roaming).await.0.unwrap();
        assert_eq!(
            *status.borrow(),
            RoamingStatus {
                reachability: Reachability::Reachable,
                position: Some(1),
                different_server: Vec::new(),
            }
        );
        // Racing after a failure keeps the last value, without a position.
        script.set(1, Probe::Fail(10));
        roaming.report(&first, AddressOutcome::Tls);
        assert_eq!(status.borrow().reachability, Reachability::Reachable);
        assert_eq!(status.borrow().position, None);
        assert_eq!(selected(&roaming).await.0, None);
        assert_eq!(status.borrow().reachability, Reachability::Unreachable);
    }

    #[tokio::test(start_paused = true)]
    async fn debug_output_names_no_address() {
        let script = Script::new(&[Probe::Pass(1)]);
        let roaming = roaming(&script);
        let before = format!("{roaming:?}");
        let selected = selected(&roaming).await.0.unwrap();
        let after = format!("{roaming:?} {selected:?}");
        for text in [&before, &after] {
            assert!(!text.contains("127.0.0.1"), "{text}");
        }
        assert!(after.contains("position: Some(1)"), "{after}");
        assert!(!format!("{:?}", network("wlan0 192.168.1.1")).contains("192.168"));
    }
}
