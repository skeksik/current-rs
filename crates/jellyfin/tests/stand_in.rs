//! Browsing the shared stand-in's fake library end to end (`tools/jellyfin-stand-in`), as the bridge tests will: each endpoint answers
//! in the shape current-jellyfin reads.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use current_jellyfin::{
    AuthError, ClientIdentity, JellyfinClient, NEXT_UP_CUTOFF, SEARCH_GROUPS, SEARCH_LIMIT,
};
use current_model::{
    CollectionKind, ImageType, ItemId, ItemQuery, ItemRef, MediaKind, ServerKey, ServerRef,
    ServiceType, SortOrder,
};
use current_net::{FixedAddress, RequestLog, Url};
use jellyfin_stand_in::library::{
    DIRECTOR_ID, EPISODE_IDS, FILM_IDS, FILMS_LIBRARY_ID, PERSON_ID, SEASON_ID, SERIES_ID,
    SHOWS_LIBRARY_ID,
};
use jellyfin_stand_in::{PASSWORD, StandIn, USER_NAME};

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

fn item(id: &str) -> ItemRef {
    ItemRef::new(server(), ItemId::new(id))
}

async fn signed_in(stand_in: &StandIn) -> JellyfinClient {
    let client = JellyfinClient::new(
        server(),
        Arc::new(FixedAddress(Url::parse(stand_in.address()).unwrap())),
        ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef"),
        Arc::new(RequestLog::default()),
    )
    .unwrap();
    let signed_in = client.sign_in_by_name(USER_NAME, PASSWORD).await.unwrap();
    client.set_token(signed_in.token);
    client
}

#[tokio::test]
async fn the_stand_ins_library_is_browsable() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in).await;
    assert!(client.latest_excludes().is_empty());

    let libraries = client.libraries().await.unwrap();
    let kinds: Vec<_> = libraries.iter().map(|l| l.collection.clone()).collect();
    assert_eq!(
        kinds,
        [Some(CollectionKind::MOVIES), Some(CollectionKind::TV_SHOWS)]
    );
    assert_eq!(libraries[0].item, item(FILMS_LIBRARY_ID));

    let films = ItemQuery {
        parent: Some(item(FILMS_LIBRARY_ID)),
        kinds: vec![MediaKind::MOVIE],
        recursive: true,
        ..ItemQuery::default()
    };
    let first = client.items(&films, 0, 2, true).await.unwrap();
    assert_eq!(first.total, Some(3));
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items[0].item, item(FILM_IDS[0]));
    assert_eq!(first.items[0].runtime, Some(Duration::from_secs(7200)));
    let second = client.items(&films, 2, 2, false).await.unwrap();
    assert_eq!((second.start, second.total), (2, None));
    assert_eq!(second.items.len(), 1);
    let descending = ItemQuery {
        order: SortOrder::Descending,
        ..films.clone()
    };
    let last_first = client.items(&descending, 0, 1, false).await.unwrap();
    assert_eq!(last_first.items[0].item, item(FILM_IDS[2]));

    let latest = client.latest(&item(SHOWS_LIBRARY_ID), 16).await.unwrap();
    assert_eq!(latest[0].item, item(SERIES_ID));
    assert_eq!(latest[0].child_count, Some(2));

    let resume = client.resume_items(12).await.unwrap();
    assert_eq!(resume[0].item, item(FILM_IDS[1]));
    assert_eq!(resume[0].user.progress, Some(0.5));

    let cutoff = SystemTime::now() - NEXT_UP_CUTOFF;
    // No episode was played: no series is in progress.
    assert!(client.next_up(12, cutoff).await.unwrap().is_empty());
    // A series' own Play button still gets its first episode.
    let first = client.series_next_up(&item(SERIES_ID)).await.unwrap();
    assert_eq!(
        first.map(|episode| episode.item),
        Some(item(EPISODE_IDS[0]))
    );

    let seasons = client.seasons(&item(SERIES_ID)).await.unwrap();
    assert_eq!(seasons[0].item, item(SEASON_ID));
    let episodes = client
        .episodes(&item(SERIES_ID), &item(SEASON_ID))
        .await
        .unwrap();
    assert_eq!(episodes.len(), 2);
    // The second episode has no image of its own: the series' poster.
    let poster = &episodes[1].artwork[0];
    assert_eq!(poster.kind, ImageType::PRIMARY);
    assert_eq!(poster.image.item, item(SERIES_ID));

    let similar = client.similar(&item(FILM_IDS[0]), 12).await.unwrap();
    assert_eq!(similar.len(), 2);

    let groups = client
        .search("episode two", SEARCH_GROUPS, SEARCH_LIMIT)
        .await
        .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].kind, MediaKind::EPISODE);
    assert_eq!(groups[0].items[0].item, item(EPISODE_IDS[1]));

    let details = client.details(&item(FILM_IDS[0])).await.unwrap();
    assert_eq!(details.genres, ["Drama"]);
    assert_eq!(details.people.len(), 1);
    let media = details.media.unwrap();
    assert_eq!(media.audio.len(), 1);
    assert!(media.video.is_some());
}

#[tokio::test]
async fn people_are_found_and_list_their_titles() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in).await;

    // Every person kind is a person: the director is found as the actor is.
    let groups = client
        .search("example", &[MediaKind::PERSON], 100)
        .await
        .unwrap();
    let people: Vec<_> = groups[0].items.iter().map(|p| p.item.clone()).collect();
    assert_eq!(people, [item(PERSON_ID), item(DIRECTOR_ID)]);
    assert_eq!(groups[0].items[0].kind, MediaKind::PERSON);
    assert_eq!(groups[0].items[0].artwork[0].kind, ImageType::PRIMARY);
    // A search for films finds no people, and no query below a library lists them.
    let films = client
        .search("example", &[MediaKind::MOVIE], 100)
        .await
        .unwrap();
    assert!(films[0].items.iter().all(|f| f.kind == MediaKind::MOVIE));
    let in_library = ItemQuery {
        parent: Some(item(FILMS_LIBRARY_ID)),
        kinds: vec![MediaKind::PERSON],
        recursive: true,
        ..ItemQuery::default()
    };
    assert!(
        client
            .items(&in_library, 0, 10, false)
            .await
            .unwrap()
            .items
            .is_empty()
    );

    let titles = ItemQuery {
        kinds: vec![MediaKind::MOVIE, MediaKind::SERIES],
        recursive: true,
        person: Some(item(PERSON_ID)),
        ..ItemQuery::default()
    };
    let page = client.items(&titles, 0, 100, true).await.unwrap();
    let found: Vec<_> = page.items.iter().map(|t| t.item.clone()).collect();
    assert_eq!(found, [item(FILM_IDS[0]), item(SERIES_ID)]);
    assert_eq!(page.total, Some(2));
}

#[tokio::test]
async fn marks_stick_and_change_next_up() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in).await;
    let first = item(EPISODE_IDS[0]);

    let state = client.set_played(&first, true).await.unwrap();
    assert!(state.played);
    assert_eq!(state.play_count, 1);
    let cutoff = SystemTime::now() - NEXT_UP_CUTOFF;
    let next_up = client.next_up(12, cutoff).await.unwrap();
    assert_eq!(next_up[0].item, item(EPISODE_IDS[1]));
    assert!(!client.set_played(&first, false).await.unwrap().played);
    // Unplayed again, nothing in the series was played.
    assert!(client.next_up(12, cutoff).await.unwrap().is_empty());

    let state = client.set_favourite(&first, true).await.unwrap();
    assert!(state.favourite);
    let details = client.details(&first).await.unwrap();
    assert!(details.summary.user.favourite);
    assert!(!client.set_favourite(&first, false).await.unwrap().favourite);
}

#[tokio::test]
async fn browsing_without_a_session_is_refused() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in).await;
    stand_in.revoke_all();
    assert_eq!(
        client.libraries().await.unwrap_err(),
        AuthError::SessionRevoked
    );
}

/// As Jellyfin 12.2 (`Folder.MarkPlayed`, `Folder.FillUserDataDtoValues`): a series' mark is one on each of its episodes, and its own
/// state, unplayed count and played percentage come from them, in the mark's answer and in `UserItems/{id}/UserData`.
#[tokio::test]
async fn a_series_mark_is_its_episodes() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in).await;
    let series = item(SERIES_ID);
    let fresh = client.user_data(&series).await.unwrap();
    assert!(!fresh.played);
    assert_eq!(fresh.unplayed_count, Some(2));
    assert_eq!(fresh.progress, Some(0.0));

    let one = client
        .set_played(&item(EPISODE_IDS[0]), true)
        .await
        .unwrap();
    assert!(one.played && one.unplayed_count.is_none());
    let half = client.user_data(&series).await.unwrap();
    assert!(!half.played);
    assert_eq!(half.unplayed_count, Some(1));
    assert_eq!(half.progress, Some(0.5));
    assert_eq!(
        client
            .user_data(&item(SEASON_ID))
            .await
            .unwrap()
            .unplayed_count,
        Some(1)
    );

    let all = client.set_played(&series, true).await.unwrap();
    assert!(all.played);
    assert_eq!(all.unplayed_count, Some(0));
    assert_eq!(all.progress, Some(1.0));
    let episodes = client.episodes(&series, &item(SEASON_ID)).await.unwrap();
    assert!(episodes.iter().all(|episode| episode.user.played));
    // Every episode played: the series has no next episode.
    let cutoff = SystemTime::now() - NEXT_UP_CUTOFF;
    assert!(client.next_up(12, cutoff).await.unwrap().is_empty());

    let none = client.set_played(&series, false).await.unwrap();
    assert!(!none.played);
    assert_eq!(none.unplayed_count, Some(2));
    let episodes = client.episodes(&series, &item(SEASON_ID)).await.unwrap();
    assert!(episodes.iter().all(|episode| !episode.user.played));
    assert!(
        client
            .user_data(&item("57a2d1e00000000000000000000fffff"))
            .await
            .is_err()
    );
}
