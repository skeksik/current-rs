//! What kind of answer a request got, for telling an authentication gateway's answer from the service's own (ADR 0017). The shapes
//! are generic: no gateway product is named or needed to decide.

use crate::challenge::AuthScheme;
use crate::client::Response;
use crate::error::{NetError, is_html};

/// The shape of an answer where the service's JSON was expected.
#[derive(Clone, PartialEq, Eq)]
pub enum GatewayShape {
    /// A success that is not an HTML page: the service answered (no gateway, or this address bypasses it). Whether it is the
    /// expected document is for the service crate to check.
    Expected,
    /// 401 or 403 with a `WWW-Authenticate: Basic` challenge: a password gateway. `realm` is the server's text, for "The site
    /// says: ..." only; Debug leaves it out.
    Password { realm: Option<String> },
    /// A web sign-in: a redirect that was not followed (another host or port), an HTML page where JSON was expected (also at the
    /// end of a followed redirect), or a 401 or 403 without a Basic challenge (a 403 only with an HTML page).
    SignIn,
    /// 403 without a challenge and without a page: the gateway wants something else (extra headers).
    Refused,
    /// 401 with challenges of other schemes only (Digest, NTLM, Negotiate, Bearer, ...).
    Unsupported,
    /// Anything else (no connection, a timeout, another status, a decode error): not a gateway's answer.
    Other,
}

impl std::fmt::Debug for GatewayShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Expected => "Expected",
            Self::Password { .. } => "Password",
            Self::SignIn => "SignIn",
            Self::Refused => "Refused",
            Self::Unsupported => "Unsupported",
            Self::Other => "Other",
        })
    }
}

/// The shape of a request's result.
pub fn classify(result: &Result<Response, NetError>) -> GatewayShape {
    match result {
        Ok(response) if is_html(&response.headers, &response.body) => GatewayShape::SignIn,
        Ok(_) => GatewayShape::Expected,
        Err(NetError::Redirected { .. }) => GatewayShape::SignIn,
        Err(NetError::Auth {
            status,
            challenges,
            page,
            ..
        }) => {
            if let Some(basic) = challenges
                .iter()
                .find(|challenge| challenge.scheme == AuthScheme::BASIC)
            {
                GatewayShape::Password {
                    realm: basic.realm.clone(),
                }
            } else if *status == 401 && !challenges.is_empty() {
                GatewayShape::Unsupported
            } else if *status == 401 || *page {
                GatewayShape::SignIn
            } else {
                GatewayShape::Refused
            }
        }
        Err(_) => GatewayShape::Other,
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
    use url::Url;

    use super::*;
    use crate::error::Location;

    fn answer(
        status: u16,
        headers: &[(&'static str, &'static str)],
        body: &str,
    ) -> Result<Response, NetError> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(*name, HeaderValue::from_static(value));
        }
        if (200..300).contains(&status) {
            Ok(Response {
                status,
                headers: map,
                body: body.as_bytes().to_vec(),
                url: Location(Url::parse("https://media.example.com/").unwrap()),
                redirected: false,
            })
        } else {
            Err(NetError::from_status(status, &map, body.as_bytes()))
        }
    }

    #[test]
    fn answers_are_sorted_into_shapes() {
        let json = [(CONTENT_TYPE.as_str(), "application/json")];
        let html = [(CONTENT_TYPE.as_str(), "text/html")];
        assert_eq!(
            classify(&answer(200, &json, r#"{"Id":"x"}"#)),
            GatewayShape::Expected
        );
        assert_eq!(
            classify(&answer(200, &[], "<!DOCTYPE html>")),
            GatewayShape::SignIn
        );
        assert_eq!(
            classify(&answer(200, &html, "sign in")),
            GatewayShape::SignIn
        );
        assert_eq!(
            classify(&answer(
                401,
                &[(WWW_AUTHENTICATE.as_str(), r#"Basic realm="Home""#)],
                ""
            )),
            GatewayShape::Password {
                realm: Some("Home".to_owned())
            }
        );
        assert_eq!(
            classify(&answer(
                401,
                &[(WWW_AUTHENTICATE.as_str(), "Negotiate, Basic")],
                ""
            )),
            GatewayShape::Password { realm: None }
        );
        assert_eq!(
            classify(&answer(
                401,
                &[(WWW_AUTHENTICATE.as_str(), "Digest realm=\"d\", Bearer")],
                ""
            )),
            GatewayShape::Unsupported
        );
        assert_eq!(
            classify(&answer(
                401,
                &[("location", "https://auth.example.com/")],
                r#"{"status":"Unauthorized"}"#
            )),
            GatewayShape::SignIn
        );
        assert_eq!(classify(&answer(401, &[], "")), GatewayShape::SignIn);
        assert_eq!(
            classify(&answer(403, &html, "<html>sign in</html>")),
            GatewayShape::SignIn
        );
        assert_eq!(classify(&answer(403, &[], "")), GatewayShape::Refused);
        assert_eq!(
            classify(&answer(403, &json, r#"{"message":"no"}"#)),
            GatewayShape::Refused
        );
        assert_eq!(
            classify(&Err(NetError::Redirected {
                status: 302,
                location: None
            })),
            GatewayShape::SignIn
        );
        assert_eq!(classify(&answer(404, &[], "")), GatewayShape::Other);
        assert_eq!(classify(&Err(NetError::Unreachable)), GatewayShape::Other);
    }

    #[test]
    fn debug_leaves_out_the_realm() {
        let shape = GatewayShape::Password {
            realm: Some("Private Realm Text".to_owned()),
        };
        assert_eq!(format!("{shape:?}"), "Password");
    }
}
