//! Fetching an item's image (ADR 0018, section 4): `GET Items/{id}/Images/{type}[/{index}]` at a box size, through the session's
//! address provider, so the address in use and its gateway credentials apply like for every other request (ADR 0014, ADR 0017).
//!
//! The URL asks for `fillWidth`/`fillHeight` (an image that covers the box at the source's aspect ratio, never upscaled),
//! `quality=90` (the server's default) and the image's `tag` (its version, which makes the answer immutable), and the request
//! accepts only JPEG and PNG, without `format`: the server then sends JPEG, or PNG when the image needs transparency, never WebP,
//! which the Windows UI cannot read (`docs/notes/jellyfin-browse.md`).

use current_model::{ImageType, ItemRef};
use current_net::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use current_net::{Request, Response};

use crate::client::{AuthError, JellyfinClient};
use crate::gateway::GatewayAnswer;

/// What [`JellyfinClient::image`] asks for, at a box size in physical pixels. The caller rounds the size to its buckets.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageRequest {
    /// Whose image: an episode's row may show its series' poster.
    pub item: ItemRef,
    pub kind: ImageType,
    /// For backdrops after the first; 0 otherwise.
    pub index: u32,
    /// The server's version of the image.
    pub tag: String,
    /// The box the image must cover. Both at least 1.
    pub width: u32,
    pub height: u32,
}

/// Why an image could not be fetched. Neither Display nor Debug holds an address, a token or an item's name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// The request failed as an authenticated request does: not signed in, a gateway's answer (its HTML page included), the
    /// network, or the server's status.
    #[error(transparent)]
    Request(#[from] AuthError),
    /// The server answered something that is not a JPEG or PNG image.
    #[error("the server's answer is not an image")]
    NotAnImage,
    /// The request cannot be sent: an id or image type that does not fit in a URL path, or an empty box.
    #[error("the image request is not valid")]
    Invalid,
}

/// Jellyfin's `ImageType` names (`Jellyfin.Data.Enums.ImageType`) for the model's known kinds. Any other kind came from the server
/// and is sent back as it is.
fn jellyfin_type(kind: &ImageType) -> &str {
    match kind {
        k if *k == ImageType::PRIMARY => "Primary",
        k if *k == ImageType::BACKDROP => "Backdrop",
        k if *k == ImageType::LOGO => "Logo",
        k if *k == ImageType::THUMB => "Thumb",
        k if *k == ImageType::BANNER => "Banner",
        other => other.as_str(),
    }
}

/// A segment that is safe in a URL path as it is: Jellyfin's ids are hex GUIDs and its image types are names.
fn path_safe(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 64
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// The query of an image request, in the order it is sent.
type Query = Vec<(&'static str, String)>;

/// The path and query for `request`.
pub(crate) fn target(request: &ImageRequest) -> Result<(String, Query), ImageError> {
    let kind = jellyfin_type(&request.kind);
    if !path_safe(request.item.id.as_str())
        || !path_safe(kind)
        || request.width == 0
        || request.height == 0
    {
        return Err(ImageError::Invalid);
    }
    let mut path = format!("/Items/{}/Images/{kind}", request.item.id.as_str());
    if request.index != 0 {
        path.push_str(&format!("/{}", request.index));
    }
    let mut query = vec![
        ("fillWidth", request.width.to_string()),
        ("fillHeight", request.height.to_string()),
        ("quality", "90".to_owned()),
    ];
    if !request.tag.is_empty() {
        query.push(("tag", request.tag.clone()));
    }
    Ok((path, query))
}

/// Whether `response` holds an image: an `image/*` content type, or none and a JPEG or PNG signature.
fn is_image(response: &Response) -> bool {
    match response
        .headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        Some(declared) => declared
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .starts_with("image/"),
        None => {
            response.body.starts_with(&[0xFF, 0xD8, 0xFF])
                || response.body.starts_with(b"\x89PNG\r\n\x1a\n")
        }
    }
}

impl JellyfinClient {
    /// Fetches an item's image at a box size and returns its encoded bytes (JPEG or PNG). Sent as an authenticated request, under
    /// the same rules (see [`JellyfinClient::send_authenticated`]); a 2xx HTML page, which a gateway's sign-in page is and an image
    /// never is, is the gateway's answer ([`GatewayAnswer::SignIn`]), not a broken image.
    ///
    /// # Errors
    /// [`ImageError`]: the request's own failure, an answer that is not an image, or a request that cannot be sent.
    pub async fn image(&self, request: &ImageRequest) -> Result<Vec<u8>, ImageError> {
        let (path, query) = target(request)?;
        let mut sent =
            Request::get(path).header(ACCEPT, HeaderValue::from_static("image/jpeg, image/png"));
        for (key, value) in query {
            sent = sent.query(key, value);
        }
        let response = self.send_authenticated(sent).await?;
        if GatewayAnswer::of(&Ok(response.clone())) == Some(GatewayAnswer::SignIn) {
            return Err(AuthError::Gateway(GatewayAnswer::SignIn).into());
        }
        if !is_image(&response) {
            return Err(ImageError::NotAnImage);
        }
        Ok(response.body)
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ItemId, ServerKey, ServerRef, ServiceType};

    use super::*;

    fn request(kind: ImageType, index: u32) -> ImageRequest {
        ImageRequest {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                ItemId::new("0123456789abcdef0123456789abcdef"),
            ),
            kind,
            index,
            tag: "e3b0c44298fc1c14".into(),
            width: 320,
            height: 480,
        }
    }

    #[test]
    fn the_url_asks_for_the_box_the_quality_and_the_version() {
        let (path, query) = target(&request(ImageType::PRIMARY, 0)).unwrap();
        assert_eq!(
            path,
            "/Items/0123456789abcdef0123456789abcdef/Images/Primary"
        );
        assert_eq!(
            query,
            [
                ("fillWidth", "320".to_owned()),
                ("fillHeight", "480".to_owned()),
                ("quality", "90".to_owned()),
                ("tag", "e3b0c44298fc1c14".to_owned()),
            ]
        );
    }

    #[test]
    fn an_index_after_the_first_goes_in_the_path() {
        let (path, _) = target(&request(ImageType::BACKDROP, 2)).unwrap();
        assert_eq!(
            path,
            "/Items/0123456789abcdef0123456789abcdef/Images/Backdrop/2"
        );
    }

    #[test]
    fn known_kinds_get_jellyfins_names_and_others_pass_through() {
        for (kind, name) in [
            (ImageType::LOGO, "Logo"),
            (ImageType::THUMB, "Thumb"),
            (ImageType::BANNER, "Banner"),
            (ImageType::new("Art"), "Art"),
        ] {
            let (path, _) = target(&request(kind, 0)).unwrap();
            assert!(path.ends_with(&format!("/Images/{name}")), "{path}");
        }
    }

    #[test]
    fn unsafe_segments_and_empty_boxes_are_refused() {
        assert_eq!(
            target(&request(ImageType::new("../Users"), 0)),
            Err(ImageError::Invalid)
        );
        let mut bad_id = request(ImageType::PRIMARY, 0);
        bad_id.item.id = ItemId::new("a/b?c");
        assert_eq!(target(&bad_id), Err(ImageError::Invalid));
        let mut empty = request(ImageType::PRIMARY, 0);
        empty.width = 0;
        assert_eq!(target(&empty), Err(ImageError::Invalid));
    }
}
