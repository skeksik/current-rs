using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.ComponentModel;
using Current.Bindings;
using Microsoft.UI.Xaml;

namespace Current.Shell.ViewModels;

/// <summary>
/// The saved servers joined with their status in the capabilities state, for Home and Settings → Accounts. It only maps the core's
/// state to display text; the core decides everything (ADR 0001). Both subscriptions live from <see cref="Start"/> to
/// <see cref="Stop"/>, which the page calls on Loaded and Unloaded (ADR 0004).
/// </summary>
public sealed class ServersViewModel(CoreHost core, IUiDispatcher ui) : INotifyPropertyChanged
{
    private ListSubscription<SavedServer>? _servers;
    private ValueSubscription<Capabilities>? _capabilities;

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>One row per saved server, in the order they were added. Rows are updated in place so focus stays on them.</summary>
    public ObservableCollection<ServerRow> Rows { get; } = [];

    public bool HasServers => Rows.Count > 0;

    /// <summary>Whether the core's first list has arrived: before it, the page shows neither the empty state nor the list.</summary>
    public bool Loaded { get; private set; }

    /// <summary>The mirror of the server list stopped after repeated failures (ListSubscription.IsFaulted); <see cref="Reload"/> helps.</summary>
    public bool Faulted { get; private set; }

    public Visibility EmptyVisibility => Loaded && !HasServers && !Faulted ? Visibility.Visible : Visibility.Collapsed;

    public Visibility ListVisibility => HasServers && !Faulted ? Visibility.Visible : Visibility.Collapsed;

    public Visibility FaultedVisibility => Faulted ? Visibility.Visible : Visibility.Collapsed;

    public void Start()
    {
        if (_servers is not null)
        {
            return;
        }
        _servers = core.ObserveServers(ui);
        _servers.Items.CollectionChanged += OnServersChanged;
        _servers.PropertyChanged += OnServersFaulted;
        _capabilities = core.ObserveCapabilities(ui);
        _capabilities.PropertyChanged += OnCapabilitiesChanged;
        Sync();
    }

    public void Stop()
    {
        if (_servers is not null)
        {
            _servers.Items.CollectionChanged -= OnServersChanged;
            _servers.PropertyChanged -= OnServersFaulted;
            _servers.Dispose();
            _servers = null;
        }
        if (_capabilities is not null)
        {
            _capabilities.PropertyChanged -= OnCapabilitiesChanged;
            _capabilities.Dispose();
            _capabilities = null;
        }
    }

    /// <summary>Subscribes anew, after the list mirror faulted.</summary>
    public void Reload()
    {
        Stop();
        Loaded = false;
        Start();
    }

    private void OnServersChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        // The first delivery is a Reset, even of an empty list.
        Loaded = true;
        Sync();
    }

    /// <summary>The mirror stopped (ListSubscription.IsFaulted): show the reload state at once.</summary>
    private void OnServersFaulted(object? sender, PropertyChangedEventArgs e) => Sync();

    private void OnCapabilitiesChanged(object? sender, PropertyChangedEventArgs e) => Sync();

    private void Sync()
    {
        if (_servers is null || _capabilities is null)
        {
            return;
        }
        Faulted = _servers.IsFaulted;
        var saved = _servers.Items;
        var statuses = _capabilities.Value.Servers;
        // Keep rows that still stand for the same server, so a status change does not rebuild the list under the user.
        for (var i = 0; i < saved.Count; i++)
        {
            var status = statuses.FirstOrDefault(s => s.Service == saved[i].Server.Service && s.Key == saved[i].Server.Key);
            if (i < Rows.Count && Rows[i].Server == saved[i].Server)
            {
                Rows[i].Update(saved[i], status);
            }
            else if (i < Rows.Count)
            {
                Rows[i] = new ServerRow(saved[i], status);
            }
            else
            {
                Rows.Add(new ServerRow(saved[i], status));
            }
        }
        while (Rows.Count > saved.Count)
        {
            Rows.RemoveAt(Rows.Count - 1);
        }
        foreach (var name in new[]
        {
            nameof(HasServers), nameof(Loaded), nameof(Faulted), nameof(EmptyVisibility), nameof(ListVisibility), nameof(FaultedVisibility),
        })
        {
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
        }
    }
}

/// <summary>One saved server as the UI shows it. Public, because x:DataType needs a public type (docs/notes/uniffi-bridge.md).</summary>
public sealed class ServerRow : INotifyPropertyChanged
{
    internal ServerRow(SavedServer saved, ServerStatus? status)
    {
        Server = saved.Server;
        Update(saved, status);
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ServerRef Server { get; }

    public string Name { get; private set; } = "";

    /// <summary>"Jellyfin 10.11.2", or "Jellyfin" before the version is known.</summary>
    public string Product { get; private set; } = "";

    /// <summary>"Signed in as …" or "Signed out".</summary>
    public string Account { get; private set; } = "";

    /// <summary>"Connected", "Can't reach the server" or "Checking…", plus "address 2 of 3" when there is more than one.</summary>
    public string Connection { get; private set; } = "";

    public bool SignedIn { get; private set; }

    public string UserName { get; private set; } = "";

    /// <summary>The buttons' names for Narrator, which a list of "Sign in" buttons would not tell apart.</summary>
    public string SignInLabel => $"Sign in to {Name}";

    public string SignOutLabel => $"Sign out of {Name}";

    public string RemoveLabel => $"Remove {Name}";

    public Visibility SignInVisibility => SignedIn ? Visibility.Collapsed : Visibility.Visible;

    public Visibility SignOutVisibility => SignedIn ? Visibility.Visible : Visibility.Collapsed;

    internal void Update(SavedServer saved, ServerStatus? status)
    {
        Name = saved.Name;
        Product = status?.Version is { Length: > 0 } version ? $"Jellyfin {version}" : "Jellyfin";
        SignedIn = status?.SignedIn ?? saved.ActiveUser is not null;
        UserName = status?.User?.Name ?? saved.Users.FirstOrDefault(user => user.Id == saved.ActiveUser)?.Name ?? "";
        Account = SignedIn
            ? (UserName.Length > 0 ? $"Signed in as {UserName}" : "Signed in")
            : "Signed out";
        var reachability = (status?.Reachability ?? Reachability.Unknown) switch
        {
            Reachability.Reachable => "Connected",
            Reachability.Unreachable => "Can't reach the server",
            _ => "Checking…",
        };
        Connection = status?.AddressPosition is { } position && saved.AddressCount > 1
            ? $"{reachability}, address {position} of {saved.AddressCount}"
            : reachability;
        foreach (var name in new[]
        {
            nameof(Name), nameof(Product), nameof(Account), nameof(Connection), nameof(SignedIn), nameof(UserName),
            nameof(SignInVisibility), nameof(SignOutVisibility), nameof(SignInLabel), nameof(SignOutLabel), nameof(RemoveLabel),
        })
        {
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
        }
    }
}
