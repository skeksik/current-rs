//! Browsing against a hand-written Jellyfin stand-in on 127.0.0.1. Answers follow Jellyfin 12.2's shapes (ADR 0018): PascalCase
//! names, nulls left out, ids as 32 hex digits. Every name is an example.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use current_jellyfin::{
    AccessToken, AuthError, ClientIdentity, JellyfinClient, NEXT_UP_CUTOFF, SEARCH_GROUPS,
    SEARCH_LIMIT,
};
use current_model::{
    AspectRatio, CalendarDate, CollectionKind, ImageType, ItemFilter, ItemId, ItemQuery, ItemRef,
    MediaKind, PersonKind, ServerKey, ServerRef, ServiceType, SortKey, SortOrder,
};
use current_net::{FixedAddress, NetError, RequestLog, Url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const MOVIE: &str = "f11m0000000000000000000000000001";
const PERSON: &str = "9e750000000000000000000000000001";
const SERIES: &str = "5e710000000000000000000000000001";
const SEASON: &str = "5ea50000000000000000000000000001";
const EPISODE: &str = "e9150000000000000000000000000001";
const LIBRARY: &str = "11b00000000000000000000000000001";

/// One request as the stand-in received it.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
}

type Handler = Arc<dyn Fn(&Received) -> (u16, String) + Send + Sync>;

struct StandIn {
    address: Url,
    received: Arc<Mutex<Vec<Received>>>,
}

impl StandIn {
    async fn start(handler: impl Fn(&Received) -> (u16, String) + Send + Sync + 'static) -> Self {
        let handler: Handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().unwrap().push(request.clone());
                    let (status, body) = handler(&request);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            address: Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
            received,
        }
    }

    /// Answers every request with `body`.
    async fn answering(body: &'static str) -> Self {
        Self::start(move |_| (200, body.to_owned())).await
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn only(&self) -> Received {
        let received = self.received();
        assert_eq!(received.len(), 1, "{received:?}");
        received[0].clone()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Received> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut first = head.lines().next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = Url::parse(&format!("http://127.0.0.1{}", first.next()?)).ok()?;
    Some(Received {
        method,
        path: target.path().to_owned(),
        query: target.query_pairs().into_owned().collect(),
    })
}

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

fn item(id: &str) -> ItemRef {
    ItemRef::new(server(), ItemId::new(id))
}

fn client(stand_in: &StandIn) -> JellyfinClient {
    let client = JellyfinClient::new(
        server(),
        Arc::new(FixedAddress(stand_in.address.clone())),
        ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef"),
        Arc::new(RequestLog::default()),
    )
    .unwrap();
    client.set_token(AccessToken::new(TOKEN));
    client
}

fn assert_summary_fields(request: &Received) {
    assert_eq!(request.query["fields"], "PrimaryImageAspectRatio");
    assert_eq!(request.query["enableImageTypes"], "Primary,Thumb,Backdrop");
    assert_eq!(request.query["imageTypeLimit"], "1");
    assert!(!request.query.contains_key("enableUserData"));
}

/// A film with every list field, an episode without its own primary image, and a kind no release of Current knows.
const ITEMS_PAGE: &str = r#"{"Items":[
  {"Name":"Example Film","ServerId":"57a2d1e0000000000000000000000001","Id":"f11m0000000000000000000000000001","RunTimeTicks":72000000000,
   "ProductionYear":2010,"IsFolder":false,"Type":"Movie","PrimaryImageAspectRatio":0.6666666666666666,
   "UserData":{"PlaybackPositionTicks":18000000000,"PlayCount":2,"IsFavorite":true,"Played":false,"PlayedPercentage":25.0,
     "Key":"f11m0000000000000000000000000001","ItemId":"f11m0000000000000000000000000001"},
   "ImageTags":{"Primary":"p1","Thumb":"t1"},"BackdropImageTags":["b1"],
   "ImageBlurHashes":{"Primary":{"p1":"LEHV6nWB2yk8pyo0adR*.7kCMdnj"},"Backdrop":{"b1":"L6PZfSi_.AyE_3t7t7R**0o#DgR4"}}},
  {"Name":"Example Episode","Id":"e9150000000000000000000000000001","RunTimeTicks":13000000000,"Type":"Episode",
   "IndexNumber":3,"IndexNumberEnd":4,"ParentIndexNumber":1,
   "SeriesName":"Example Series","SeriesId":"5e710000000000000000000000000001",
   "SeasonId":"5ea50000000000000000000000000001","SeasonName":"Season 1",
   "SeriesPrimaryImageTag":"sp1","PrimaryImageAspectRatio":0.68,
   "ImageBlurHashes":{"Primary":{"sp1":"LKO2?U%2Tw=w]~RBVZRi};RPxuwH"}},
   "UserData":{"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":false,"Played":true,"Key":"x","ItemId":"x"},
   "ImageTags":{},"BackdropImageTags":[]},
  {"Name":"Example Future Thing","Id":"0f070000000000000000000000000001","Type":"SomeFutureKind"},
  {"Name":"No id"}
],"TotalRecordCount":1234,"StartIndex":100}"#;

#[tokio::test]
async fn items_send_the_query_and_page() {
    let stand_in = StandIn::answering(ITEMS_PAGE).await;
    let client = client(&stand_in);
    let query = ItemQuery {
        parent: Some(item(LIBRARY)),
        kinds: vec![MediaKind::MOVIE, MediaKind::new("Trailer")],
        recursive: true,
        sort: vec![SortKey::DATE_CREATED],
        order: SortOrder::Descending,
        filters: vec![ItemFilter::UNPLAYED, ItemFilter::new("Likes")],
        genres: vec!["Drama".into(), "Action, Adventure".into()],
        years: vec![2009, 2010],
        name_starts_with: Some("E".into()),
        search_term: Some("Example".into()),
        person: Some(item(PERSON)),
    };
    let page = client.items(&query, 100, 100, true).await.unwrap();

    let request = stand_in.only();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/Items");
    assert_summary_fields(&request);
    let q = &request.query;
    assert_eq!(q["parentId"], LIBRARY);
    assert_eq!(q["includeItemTypes"], "Movie,Trailer");
    assert_eq!(q["recursive"], "true");
    assert_eq!(q["sortBy"], "DateCreated,SortName");
    assert_eq!(q["sortOrder"], "Descending");
    assert_eq!(q["filters"], "IsUnplayed,Likes");
    assert_eq!(q["genres"], "Drama|Action, Adventure");
    assert_eq!(q["years"], "2009,2010");
    assert_eq!(q["nameStartsWith"], "E");
    assert_eq!(q["searchTerm"], "Example");
    assert_eq!(q["personIds"], PERSON);
    assert_eq!(q["startIndex"], "100");
    assert_eq!(q["limit"], "100");
    assert_eq!(q["enableTotalRecordCount"], "true");

    assert_eq!(page.total, Some(1234));
    assert_eq!(page.start, 100);
    // The item without an id is dropped; the unknown kind passes through.
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.items[2].kind.as_str(), "SomeFutureKind");
    assert!(!page.items[2].kind.is_known());

    let film = &page.items[0];
    assert_eq!(film.item, item(MOVIE));
    assert_eq!(film.kind, MediaKind::MOVIE);
    assert_eq!(film.name, "Example Film");
    assert_eq!(film.year, Some(2010));
    assert_eq!(film.runtime, Some(Duration::from_secs(7200)));
    assert_eq!(film.user.position, Some(Duration::from_secs(1800)));
    assert_eq!(film.user.progress, Some(0.25));
    assert!(film.user.favourite);
    assert!(!film.user.played);
    assert_eq!(film.user.play_count, 2);
    assert_eq!(film.episode, None);
    let kinds: Vec<_> = film.artwork.iter().map(|a| a.kind.clone()).collect();
    assert_eq!(
        kinds,
        [ImageType::PRIMARY, ImageType::THUMB, ImageType::BACKDROP]
    );
    let primary = &film.artwork[0];
    assert_eq!(primary.image.item, item(MOVIE));
    assert_eq!(primary.image.tag, "p1");
    assert_eq!(primary.image.index, 0);
    assert_eq!(primary.aspect_ratio, AspectRatio::new(2, 3));
    assert_eq!(
        primary.blurhash.as_deref(),
        Some("LEHV6nWB2yk8pyo0adR*.7kCMdnj")
    );
    assert_eq!(film.artwork[1].blurhash, None);
    assert_eq!(film.artwork[2].image.tag, "b1");
    assert!(film.artwork[2].blurhash.is_some());

    let episode = &page.items[1];
    assert_eq!(episode.item, item(EPISODE));
    let links = episode.episode.as_ref().unwrap();
    assert_eq!(links.series, Some(item(SERIES)));
    assert_eq!(links.series_name.as_deref(), Some("Example Series"));
    assert_eq!(links.season, Some(item(SEASON)));
    assert_eq!(links.season_name.as_deref(), Some("Season 1"));
    assert_eq!(
        (links.number, links.number_end, links.season_number),
        (Some(3), Some(4), Some(1))
    );
    assert!(episode.user.played);
    assert_eq!(episode.user.position, None);
    // No image of its own: the series' poster, as the series' image.
    assert_eq!(episode.artwork.len(), 1);
    let poster = &episode.artwork[0];
    assert_eq!(poster.kind, ImageType::PRIMARY);
    assert_eq!(poster.image.item, item(SERIES));
    assert_eq!(poster.image.tag, "sp1");
    assert_eq!(
        poster.blurhash.as_deref(),
        Some("LKO2?U%2Tw=w]~RBVZRi};RPxuwH")
    );
    assert_eq!(poster.aspect_ratio, AspectRatio::new(17, 25));
}

#[tokio::test]
async fn later_pages_skip_the_total_and_defaults_stay_out() {
    let stand_in =
        StandIn::answering(r#"{"Items":[],"TotalRecordCount":0,"StartIndex":200}"#).await;
    let client = client(&stand_in);
    let page = client
        .items(&ItemQuery::default(), 200, 100, false)
        .await
        .unwrap();
    assert_eq!(page.total, None);
    assert_eq!(page.start, 200);
    assert!(page.items.is_empty());
    let q = stand_in.only().query;
    assert_eq!(q["enableTotalRecordCount"], "false");
    assert_eq!(q["sortBy"], "SortName");
    assert_eq!(q["sortOrder"], "Ascending");
    assert_eq!(q["recursive"], "false");
    for absent in [
        "parentId",
        "includeItemTypes",
        "filters",
        "genres",
        "years",
        "nameStartsWith",
        "searchTerm",
    ] {
        assert!(!q.contains_key(absent), "{absent}");
    }
}

#[tokio::test]
async fn a_search_without_sort_keys_keeps_the_server_s_relevance_order() {
    let stand_in = StandIn::answering(r#"{"Items":[],"TotalRecordCount":0,"StartIndex":0}"#).await;
    let client = client(&stand_in);
    let searching = ItemQuery {
        parent: Some(item(LIBRARY)),
        search_term: Some("Example".into()),
        ..ItemQuery::default()
    };
    client.items(&searching, 0, 100, true).await.unwrap();
    // A search with sort keys is sorted as asked.
    let sorted = ItemQuery {
        sort: vec![SortKey::DATE_CREATED],
        order: SortOrder::Descending,
        ..searching.clone()
    };
    client.items(&sorted, 0, 100, true).await.unwrap();

    let received = stand_in.received();
    let relevance = &received[0].query;
    assert_eq!(relevance["searchTerm"], "Example");
    assert!(!relevance.contains_key("sortBy"), "{relevance:?}");
    assert!(!relevance.contains_key("sortOrder"), "{relevance:?}");
    let ordered = &received[1].query;
    assert_eq!(ordered["sortBy"], "DateCreated,SortName");
    assert_eq!(ordered["sortOrder"], "Descending");
}

#[tokio::test]
async fn missing_fields_are_not_errors() {
    let stand_in =
        StandIn::answering(r#"{"Items":[{"Id":"f11m0000000000000000000000000001"}]}"#).await;
    let page = client(&stand_in)
        .items(&ItemQuery::default(), 0, 100, true)
        .await
        .unwrap();
    assert_eq!(page.total, None);
    let only = &page.items[0];
    assert_eq!(only.name, "");
    assert_eq!(only.kind.as_str(), "");
    assert_eq!(only.year, None);
    assert_eq!(only.runtime, None);
    assert!(only.artwork.is_empty());
    assert_eq!(only.user, Default::default());
    assert_eq!(only.child_count, None);
}

#[tokio::test]
async fn libraries_keep_the_servers_order_and_pass_unknown_types_through() {
    let stand_in = StandIn::answering(
        r#"{"Items":[
          {"Name":"Example Films","Id":"11b00000000000000000000000000001","CollectionType":"movies","Type":"CollectionFolder",
           "ImageTags":{"Primary":"lp1"},"BackdropImageTags":[]},
          {"Name":"Example Shows","Id":"11b00000000000000000000000000002","CollectionType":"tvshows","Type":"CollectionFolder"},
          {"Name":"Example Mixed","Id":"11b00000000000000000000000000003","Type":"CollectionFolder"},
          {"Name":"Example Channels","Id":"11b00000000000000000000000000004","CollectionType":"livetv","Type":"UserView"}
        ],"TotalRecordCount":4,"StartIndex":0}"#,
    )
    .await;
    let libraries = client(&stand_in).libraries().await.unwrap();
    let request = stand_in.only();
    assert_eq!(request.path, "/UserViews");
    // With the libraries kept out of "My media": the navigation shows them hidden (ADR 0024).
    assert_eq!(request.query["includeHidden"], "true");
    let names: Vec<_> = libraries.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Example Films",
            "Example Shows",
            "Example Mixed",
            "Example Channels"
        ]
    );
    assert_eq!(libraries[0].collection, Some(CollectionKind::MOVIES));
    assert_eq!(libraries[0].item, item(LIBRARY));
    assert_eq!(libraries[0].artwork[0].image.tag, "lp1");
    assert_eq!(libraries[1].collection, Some(CollectionKind::TV_SHOWS));
    assert_eq!(libraries[2].collection, None);
    let unknown = libraries[3].collection.as_ref().unwrap();
    assert_eq!(unknown.as_str(), "livetv");
    assert!(!unknown.is_known());
}

#[tokio::test]
async fn latest_is_a_bare_array_with_grouped_episodes() {
    let stand_in = StandIn::answering(
        r#"[
          {"Name":"Example Series","Id":"5e710000000000000000000000000001","Type":"Series","ChildCount":3,
           "UserData":{"UnplayedItemCount":3,"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":false,"Played":false,"Key":"k","ItemId":"i"},
           "ImageTags":{"Primary":"sp1"}},
          {"Name":"Example Film","Id":"f11m0000000000000000000000000001","Type":"Movie"}
        ]"#,
    )
    .await;
    let latest = client(&stand_in).latest(&item(LIBRARY), 16).await.unwrap();
    let request = stand_in.only();
    assert_eq!(request.path, "/Items/Latest");
    assert_summary_fields(&request);
    assert_eq!(request.query["parentId"], LIBRARY);
    assert_eq!(request.query["limit"], "16");
    assert_eq!(request.query["groupItems"], "true");
    assert_eq!(latest.len(), 2);
    assert_eq!(latest[0].kind, MediaKind::SERIES);
    assert_eq!(latest[0].child_count, Some(3));
    assert_eq!(latest[0].user.unplayed_count, Some(3));
    assert_eq!(latest[1].child_count, None);
}

#[tokio::test]
async fn resume_and_next_up_ask_for_their_rows() {
    let stand_in = StandIn::answering(ITEMS_PAGE).await;
    let client = client(&stand_in);
    let resume = client.resume_items(12).await.unwrap();
    assert_eq!(resume.len(), 3);
    let cutoff = UNIX_EPOCH + Duration::from_secs(1_793_000_000) - NEXT_UP_CUTOFF;
    client.next_up(12, cutoff).await.unwrap();

    let received = stand_in.received();
    let resume = &received[0];
    assert_eq!(resume.path, "/UserItems/Resume");
    assert_summary_fields(resume);
    assert_eq!(resume.query["limit"], "12");
    assert_eq!(resume.query["mediaTypes"], "Video");
    assert_eq!(resume.query["enableTotalRecordCount"], "false");
    let next_up = &received[1];
    assert_eq!(next_up.path, "/Shows/NextUp");
    assert_summary_fields(next_up);
    assert_eq!(next_up.query["enableResumable"], "false");
    assert_eq!(next_up.query["enableTotalRecordCount"], "false");
    // 365 days before 2026-10-26T07:33:20Z.
    assert_eq!(next_up.query["nextUpDateCutoff"], "2025-10-26T07:33:20Z");
}

#[tokio::test]
async fn a_series_play_asks_for_its_next_and_first_episode() {
    let stand_in = StandIn::answering(ITEMS_PAGE).await;
    let client = client(&stand_in);
    let next = client.series_next_up(&item(SERIES)).await.unwrap();
    let first = client.first_episode(&item(SERIES)).await.unwrap();
    // The first item of the answer, either way.
    assert!(next.is_some() && next == first);

    let received = stand_in.received();
    let next_up = &received[0];
    assert_eq!(next_up.path, "/Shows/NextUp");
    assert_summary_fields(next_up);
    assert_eq!(next_up.query["seriesId"], SERIES);
    assert_eq!(next_up.query["limit"], "1");
    // An episode in progress is the series' next one: the server's default for enableResumable (true) stays, and no cutoff.
    assert!(!next_up.query.contains_key("enableResumable"));
    assert!(!next_up.query.contains_key("nextUpDateCutoff"));
    let episodes = &received[1];
    assert_eq!(episodes.path, format!("/Shows/{SERIES}/Episodes"));
    assert_summary_fields(episodes);
    // Every season, without the episodes the server only knows of (shown to a user who displays missing episodes).
    assert!(!episodes.query.contains_key("seasonId"));
    assert_eq!(episodes.query["isMissing"], "false");
    assert_eq!(episodes.query["startIndex"], "0");
    assert_eq!(episodes.query["limit"], "50");
}

/// An episode answer: `season` is its season number (0: a special).
fn episode_json(id: u32, season: u32) -> String {
    format!(
        r#"{{"Id":"e9150000000000000000000000{id:06}","Name":"Example Episode","Type":"Episode","ParentIndexNumber":{season},"IndexNumber":1}}"#
    )
}

#[tokio::test]
async fn a_series_first_episode_passes_its_specials() {
    // 50 specials first, as the server lists Specials (season 0) before season 1; then season 1.
    let stand_in = StandIn::start(|request| {
        let start: u32 = request.query["startIndex"].parse().unwrap();
        let items: Vec<String> = match start {
            0 => (0..50).map(|id| episode_json(id, 0)).collect(),
            _ => vec![episode_json(50, 0), episode_json(51, 1)],
        };
        (
            200,
            format!(r#"{{"Items":[{}],"TotalRecordCount":52}}"#, items.join(",")),
        )
    })
    .await;
    let first = client(&stand_in)
        .first_episode(&item(SERIES))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.item.id.as_str(), "e9150000000000000000000000000051");
    let received = stand_in.received();
    assert_eq!(received.len(), 2);
    assert_eq!(received[1].query["startIndex"], "50");
}

#[tokio::test]
async fn a_series_with_only_specials_plays_its_first() {
    let stand_in = StandIn::start(|_| {
        let items = [episode_json(1, 0), episode_json(2, 0)].join(",");
        (
            200,
            format!(r#"{{"Items":[{items}],"TotalRecordCount":2}}"#),
        )
    })
    .await;
    let first = client(&stand_in)
        .first_episode(&item(SERIES))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.item.id.as_str(), "e9150000000000000000000000000001");
    assert_eq!(stand_in.received().len(), 1);
}

#[tokio::test]
async fn seasons_episodes_and_similar() {
    let stand_in = StandIn::answering(ITEMS_PAGE).await;
    let client = client(&stand_in);
    client.seasons(&item(SERIES)).await.unwrap();
    client.episodes(&item(SERIES), &item(SEASON)).await.unwrap();
    client.similar(&item(MOVIE), 12).await.unwrap();

    let received = stand_in.received();
    assert_eq!(received[0].path, format!("/Shows/{SERIES}/Seasons"));
    assert_summary_fields(&received[0]);
    assert_eq!(received[1].path, format!("/Shows/{SERIES}/Episodes"));
    assert_summary_fields(&received[1]);
    assert_eq!(received[1].query["seasonId"], SEASON);
    assert_eq!(received[2].path, format!("/Items/{MOVIE}/Similar"));
    assert_eq!(received[2].query["fields"], "PrimaryImageAspectRatio");
    assert_eq!(received[2].query["limit"], "12");
}

#[tokio::test]
async fn search_sends_one_request_per_group_and_drops_empty_ones() {
    let stand_in = StandIn::start(|request| {
        let body = match request.query["includeItemTypes"].as_str() {
            "Movie" => r#"{"Items":[{"Id":"f11m0000000000000000000000000001","Name":"Example Film","Type":"Movie"}],"TotalRecordCount":0}"#,
            "Episode" => r#"{"Items":[{"Id":"e9150000000000000000000000000001","Name":"Example Episode","Type":"Episode"}],"TotalRecordCount":0}"#,
            _ => r#"{"Items":[],"TotalRecordCount":0}"#,
        };
        (200, body.to_owned())
    })
    .await;
    let groups = client(&stand_in)
        .search("example", SEARCH_GROUPS, SEARCH_LIMIT)
        .await
        .unwrap();

    let received = stand_in.received();
    assert_eq!(received.len(), SEARCH_GROUPS.len());
    let mut asked: Vec<_> = received
        .iter()
        .map(|r| r.query["includeItemTypes"].clone())
        .collect();
    asked.sort();
    let mut expected = vec![
        "Audio",
        "BoxSet",
        "Episode",
        "Movie",
        "MusicAlbum",
        "MusicArtist",
        "MusicVideo",
        "Person",
        "Series",
        "Video",
    ];
    expected.sort();
    assert_eq!(asked, expected);
    for request in &received {
        assert_eq!(request.path, "/Items");
        assert_summary_fields(request);
        assert_eq!(request.query["searchTerm"], "example");
        assert_eq!(request.query["recursive"], "true");
        assert_eq!(request.query["limit"], "24");
        assert_eq!(request.query["enableTotalRecordCount"], "false");
        assert!(!request.query.contains_key("startIndex"));
    }
    let kinds: Vec<_> = groups.iter().map(|g| g.kind.clone()).collect();
    assert_eq!(kinds, [MediaKind::MOVIE, MediaKind::EPISODE]);
    assert_eq!(groups[0].items[0].name, "Example Film");
}

/// A person as `Items?includeItemTypes=Person` answers one: a by-name item's `BaseItemDto` (`DtoService.GetBaseItemDtos`; jellyfin/jellyfin
/// v12.2, ca0f16eb, read 2026-10-27), with its picture as the primary image. Hand-written: the fields are those the code reads.
const PEOPLE: &str = r#"{"Items":[
  {"Name":"Example Actor","ServerId":"5e70000000000000000000000000001","Id":"9e750000000000000000000000000001","Type":"Person",
   "IsFolder":false,"PrimaryImageAspectRatio":0.6666666666666666,"ImageTags":{"Primary":"pp1"},"BackdropImageTags":[],
   "ImageBlurHashes":{"Primary":{"pp1":"LHB:Q@t7_4WB~qt7%Mt7-;t7Rjj["}},
   "UserData":{"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":false,"Played":false,"Key":"Person-Example Actor"}},
  {"Name":"Example Director","Id":"9e750000000000000000000000000002","Type":"Person","ImageTags":{},"BackdropImageTags":[]}
],"TotalRecordCount":0,"StartIndex":0}"#;

#[tokio::test]
async fn people_are_searched_as_items_and_map_to_summaries() {
    let stand_in = StandIn::answering(PEOPLE).await;
    let groups = client(&stand_in)
        .search("example", &[MediaKind::PERSON], 100)
        .await
        .unwrap();
    let request = stand_in.only();
    assert_eq!(request.path, "/Items");
    assert_eq!(request.query["includeItemTypes"], "Person");
    assert_eq!(request.query["limit"], "100");
    assert!(!request.query.contains_key("parentId"));
    assert_eq!(groups.len(), 1);
    let people = &groups[0].items;
    assert_eq!(people.len(), 2);
    assert_eq!(people[0].kind, MediaKind::PERSON);
    assert_eq!(people[0].name, "Example Actor");
    let picture = &people[0].artwork[0];
    assert_eq!(picture.kind, ImageType::PRIMARY);
    assert_eq!(picture.image.item, item(PERSON));
    assert!(picture.blurhash.is_some());
    assert!(people[1].artwork.is_empty());
}

#[tokio::test]
async fn a_failed_search_group_fails_the_search() {
    let stand_in = StandIn::start(|request| {
        if request.query["includeItemTypes"] == "Series" {
            (500, String::new())
        } else {
            (200, r#"{"Items":[]}"#.to_owned())
        }
    })
    .await;
    let error = client(&stand_in)
        .search("example", SEARCH_GROUPS, SEARCH_LIMIT)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            AuthError::Network(NetError::Status { status: 500, .. })
        ),
        "{error:?}"
    );
}

const DETAILS: &str = r#"{"Name":"Example Film","OriginalTitle":"Example Original Title","Id":"f11m0000000000000000000000000001",
  "Type":"Movie","ProductionYear":2010,"RunTimeTicks":72000000000,"Container":"mkv",
  "Overview":"An example overview.","Taglines":["An example tagline."],"Genres":["Drama","Science Fiction"],
  "Studios":[{"Name":"Example Studio","Id":"57d10000000000000000000000000001"}],
  "People":[
    {"Name":"Example Actor","Id":"9e750000000000000000000000000001","Role":"Example Role","Type":"Actor","PrimaryImageTag":"pp1",
     "ImageBlurHashes":{"Primary":{"pp1":"LHB:Q@t7_4WB~qt7%Mt7-;t7Rjj["}}},
    {"Name":"Example Director","Id":"9e750000000000000000000000000002","Role":"","Type":"Director"},
    {"Name":"Example Lyricist","Id":"9e750000000000000000000000000003","Type":"Lyricist"}
  ],
  "OfficialRating":"PG-13","CommunityRating":8.4,"CriticRating":87,
  "PremiereDate":"2010-07-16T00:00:00.0000000Z","EndDate":"2010-07-16T00:00:00.0000000Z","Status":"Released",
  "ImageTags":{"Primary":"p1","Logo":"l1"},"BackdropImageTags":["b1","b2"],
  "ImageBlurHashes":{"Backdrop":{"b2":"LGF5]+Yk^6#M@-5c,1J5@[or[Q6."}},
  "Chapters":[{"StartPositionTicks":0,"Name":"Chapter 1","ImageDateModified":"0001-01-01T00:00:00.0000000Z"},
              {"StartPositionTicks":3000000000,"Name":"Chapter 2","ImageDateModified":"0001-01-01T00:00:00.0000000Z"}],
  "MediaSourceCount":2,
  "MediaSources":[{"Id":"f11m0000000000000000000000000001","Container":"mkv","MediaStreams":[
    {"Codec":"hevc","Type":"Video","DisplayTitle":"4K HEVC HDR","Index":0},
    {"Codec":"eac3","Language":"eng","Type":"Audio","DisplayTitle":"English - Dolby Digital+ - 5.1 - Default","Index":1},
    {"Codec":"aac","Language":"fra","Type":"Audio","DisplayTitle":"French - AAC - Stereo","Index":2},
    {"Codec":"subrip","Language":"eng","Type":"Subtitle","DisplayTitle":"English - SUBRIP","Index":3},
    {"Codec":"mjpeg","Type":"EmbeddedImage","Index":4}
  ]}],
  "UserData":{"PlaybackPositionTicks":0,"PlayCount":1,"IsFavorite":false,"Played":true,"Key":"k","ItemId":"i"}}"#;

#[tokio::test]
async fn details_map_every_field() {
    let stand_in = StandIn::start(|request| match request.path.as_str() {
        "/Items" => (500, String::new()),
        _ => (200, DETAILS.to_owned()),
    })
    .await;
    let details = client(&stand_in).details(&item(MOVIE)).await.unwrap();
    let mut received = stand_in.received();
    received.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(received.len(), 2, "{received:?}");
    assert_eq!(received[1].path, format!("/Items/{MOVIE}"));
    assert!(received[1].query.is_empty());
    // The marks as a list reads them: it failed here, so the item's own stand.
    assert_eq!(received[0].path, "/Items");
    assert_eq!(
        received[0].query.get("ids").map(String::as_str),
        Some(MOVIE)
    );

    assert_eq!(details.summary.name, "Example Film");
    assert!(details.summary.user.played);
    assert_eq!(
        details.original_title.as_deref(),
        Some("Example Original Title")
    );
    assert_eq!(details.overview.as_deref(), Some("An example overview."));
    assert_eq!(details.taglines, ["An example tagline."]);
    assert_eq!(details.genres, ["Drama", "Science Fiction"]);
    assert_eq!(details.studios, ["Example Studio"]);
    assert_eq!(details.official_rating.as_deref(), Some("PG-13"));
    assert_eq!(details.community_rating, Some(8.4));
    assert_eq!(details.critic_rating, Some(87.0));
    assert_eq!(details.premiere, CalendarDate::new(2010, 7, 16));
    assert_eq!(details.status.as_deref(), Some("Released"));

    assert_eq!(details.people.len(), 3);
    let actor = &details.people[0];
    assert_eq!(actor.kind, PersonKind::ACTOR);
    assert_eq!(actor.role.as_deref(), Some("Example Role"));
    let image = actor.image.as_ref().unwrap();
    assert_eq!(image.image.item, item("9e750000000000000000000000000001"));
    assert_eq!(image.image.tag, "pp1");
    assert!(image.blurhash.is_some());
    assert_eq!(details.people[1].kind, PersonKind::DIRECTOR);
    assert_eq!(details.people[1].role, None);
    assert_eq!(details.people[1].image, None);
    assert_eq!(details.people[2].kind.as_str(), "Lyricist");

    let backdrops: Vec<_> = details
        .backdrops
        .iter()
        .map(|b| (b.image.index, b.image.tag.as_str(), b.blurhash.is_some()))
        .collect();
    assert_eq!(backdrops, [(0, "b1", false), (1, "b2", true)]);
    let logo = details.logo.as_ref().unwrap();
    assert_eq!(logo.kind, ImageType::LOGO);
    assert_eq!(logo.image.tag, "l1");

    assert_eq!(details.chapters.len(), 2);
    assert_eq!(details.chapters[1].start, Duration::from_secs(300));
    assert_eq!(details.chapters[1].name.as_deref(), Some("Chapter 2"));

    let media = details.media.as_ref().unwrap();
    assert_eq!(media.source_count, 2);
    assert_eq!(media.container.as_deref(), Some("mkv"));
    let video = media.video.as_ref().unwrap();
    assert_eq!(video.label, "4K HEVC HDR");
    assert_eq!(video.codec.as_deref(), Some("hevc"));
    assert_eq!(media.audio.len(), 2);
    assert_eq!(media.audio[1].language.as_deref(), Some("fra"));
    assert_eq!(media.subtitles.len(), 1);
    assert_eq!(media.subtitles[0].label, "English - SUBRIP");
}

/// The single-item read answers the server's cached instance of the item, which a mark on its series does not save (Jellyfin v12.3,
/// `LibraryManager.GetItemById`): the marks come from a list read of it, as its other fields do not.
#[tokio::test]
async fn details_take_their_marks_from_a_list_read() {
    let stand_in = StandIn::start(|request| match request.path.as_str() {
        "/Items" => (
            200,
            format!(
                r#"{{"Items":[{{"Name":"Example Film","Id":"{MOVIE}","Type":"Movie",
                    "UserData":{{"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":true,"Played":false}}}}]}}"#
            ),
        ),
        _ => (200, DETAILS.to_owned()),
    })
    .await;
    let details = client(&stand_in).details(&item(MOVIE)).await.unwrap();
    assert!(!details.summary.user.played, "the list's marks");
    assert!(details.summary.user.favourite);
    assert_eq!(details.overview.as_deref(), Some("An example overview."));

    // A list that leaves the item out changes nothing.
    let stand_in = StandIn::start(|request| match request.path.as_str() {
        "/Items" => (200, r#"{"Items":[]}"#.to_owned()),
        _ => (200, DETAILS.to_owned()),
    })
    .await;
    let details = client(&stand_in).details(&item(MOVIE)).await.unwrap();
    assert!(details.summary.user.played);

    // The item's own read failing fails the details.
    let stand_in = StandIn::start(|request| match request.path.as_str() {
        "/Items" => (200, r#"{"Items":[]}"#.to_owned()),
        _ => (500, String::new()),
    })
    .await;
    assert!(client(&stand_in).details(&item(MOVIE)).await.is_err());
}

#[tokio::test]
async fn details_of_a_series_have_no_media() {
    let stand_in = StandIn::answering(
        r#"{"Name":"Example Series","Id":"5e710000000000000000000000000001","Type":"Series","Status":"Continuing",
            "EndDate":null,"Genres":null,"People":[]}"#,
    )
    .await;
    let details = client(&stand_in).details(&item(SERIES)).await.unwrap();
    assert_eq!(details.media, None);
    assert_eq!(details.status.as_deref(), Some("Continuing"));
    assert_eq!(details.end, None);
    assert!(details.genres.is_empty());
    assert!(details.backdrops.is_empty());
    assert_eq!(details.logo, None);
}

const USER_DATA: &str = r#"{"PlayedPercentage":0,"PlaybackPositionTicks":0,"PlayCount":1,"IsFavorite":true,"Played":true,
  "LastPlayedDate":"2026-10-25T07:00:00.0000000Z","Key":"f11m0000000000000000000000000001","ItemId":"f11m0000000000000000000000000001"}"#;

#[tokio::test]
async fn played_and_favourite_answer_the_user_data() {
    let stand_in = StandIn::answering(USER_DATA).await;
    let client = client(&stand_in);
    let state = client.set_played(&item(MOVIE), true).await.unwrap();
    assert!(state.played);
    assert!(state.favourite);
    assert_eq!(state.play_count, 1);
    assert_eq!(state.position, None);
    assert_eq!(state.progress, Some(0.0));
    client.set_played(&item(MOVIE), false).await.unwrap();
    client.set_favourite(&item(MOVIE), true).await.unwrap();
    client.set_favourite(&item(MOVIE), false).await.unwrap();

    let calls: Vec<_> = stand_in
        .received()
        .into_iter()
        .map(|r| (r.method, r.path))
        .collect();
    let played = format!("/UserPlayedItems/{MOVIE}");
    let favourite = format!("/UserFavoriteItems/{MOVIE}");
    assert_eq!(
        calls,
        [
            ("POST".to_owned(), played.clone()),
            ("DELETE".to_owned(), played),
            ("POST".to_owned(), favourite.clone()),
            ("DELETE".to_owned(), favourite),
        ]
    );
}

#[tokio::test]
async fn browsing_needs_a_token() {
    let stand_in = StandIn::answering("{}").await;
    let client = client(&stand_in);
    client.clear_token();
    assert_eq!(
        client.libraries().await.unwrap_err(),
        AuthError::NotSignedIn
    );
    assert!(stand_in.received().is_empty());
}

#[tokio::test]
async fn the_session_keeps_the_libraries_left_out_of_latest() {
    let stand_in = StandIn::answering(
        r#"{"Name":"Example User","Id":"5d1c0000000000000000000000000001","Policy":{},
            "Configuration":{"LatestItemsExcludes":["11b00000000000000000000000000002"],"HidePlayedInLatest":true}}"#,
    )
    .await;
    let client = client(&stand_in);
    assert!(client.latest_excludes().is_empty());
    client.current_user().await.unwrap();
    assert_eq!(
        client.latest_excludes(),
        [item("11b00000000000000000000000000002")]
    );
    client.clear_token();
    assert!(client.latest_excludes().is_empty());
}

#[tokio::test]
async fn the_session_keeps_the_libraries_left_out_of_my_media() {
    let stand_in = StandIn::answering(
        r#"{"Name":"Example User","Id":"5d1c0000000000000000000000000001","Policy":{},
            "Configuration":{"MyMediaExcludes":["11b00000000000000000000000000003",""],"OrderedViews":[]}}"#,
    )
    .await;
    let client = client(&stand_in);
    assert!(client.hidden_libraries().is_empty());
    client.current_user().await.unwrap();
    assert_eq!(
        client.hidden_libraries(),
        [item("11b00000000000000000000000000003")]
    );
    client.clear_token();
    assert!(client.hidden_libraries().is_empty());
}
