//! Spotify's authorization-code flow with the user's own client: the callback is a plugin route.
use crate::model::{Settings, required};
use crate::net::Net;
use serde_json::Value;

const AUTHORIZE: &str = "https://accounts.spotify.com/authorize";
const TOKEN: &str = "https://accounts.spotify.com/api/token";
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-library-read";

fn basic(id: &str, secret: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{id}:{secret}").into_bytes();
    let mut out = String::from("Basic ");
    for c in raw.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= c.len() {
                T[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

pub fn authorize_url(st: &Settings, state: &str) -> Result<String, String> {
    let id = required(st, "clientId")?;
    let redirect = required(st, "redirectUri")?;
    url::Url::parse_with_params(
        AUTHORIZE,
        &[
            ("response_type", "code"),
            ("client_id", &id),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("state", state),
        ],
    )
    .map(Into::into)
    .map_err(|_| "not a valid URL".into())
}

fn grant(net: &impl Net, st: &Settings, form: &[(&str, &str)]) -> Result<Value, String> {
    let auth = basic(&required(st, "clientId")?, &required(st, "clientSecret")?);
    net.post_form(TOKEN, &[("authorization", auth)], form)
}

/// The refresh token a callback `code` is worth.
pub fn exchange(net: &impl Net, st: &Settings, code: &str) -> Result<String, String> {
    let redirect = required(st, "redirectUri")?;
    let v = grant(
        net,
        st,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
        ],
    )?;
    v["refresh_token"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "no refresh token".into())
}

/// An access token and the refresh token to keep: Spotify may rotate it.
pub fn refresh(
    net: &impl Net,
    st: &Settings,
    refresh_token: &str,
) -> Result<(String, String), String> {
    let v = grant(
        net,
        st,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )?;
    let access = v["access_token"]
        .as_str()
        .ok_or("no access token")?
        .to_owned();
    let next = v["refresh_token"]
        .as_str()
        .map_or_else(|| refresh_token.to_owned(), str::to_owned);
    Ok((access, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_is_base64() {
        assert_eq!(
            basic("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(basic("a", "b"), "Basic YTpi");
    }

    #[test]
    fn the_authorize_url_names_the_callback_and_the_state() {
        let st: Settings =
            serde_json::from_str(r#"{"clientId":"cid","redirectUri":"https://f.example/cb"}"#)
                .unwrap();
        let u = authorize_url(&st, "s1").unwrap();
        assert!(u.starts_with(AUTHORIZE) && u.contains("client_id=cid") && u.contains("state=s1"));
        assert!(u.contains("redirect_uri=https%3A%2F%2Ff.example%2Fcb"));
        assert!(authorize_url(&Settings::new(), "s").is_err());
    }
}
