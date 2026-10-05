//! Gateway-issued console token (forward-compatible): with a gateway URL and no token supplied,
//! ask `POST {gw}/api/console/token` for a short-lived one scoped to the project. The answer is
//! held in memory only (never written to storage); if the gateway has no such endpoint the
//! console falls back to the manual token field.

use super::*;

/// What the gateway issued: `{"token": "...", "expires_at": <unix seconds>}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Minted {
    pub token: String,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum MintState {
    #[default]
    Idle,
    Pending,
    Done(Minted),
    /// Not issued (404 = the gateway does not offer it yet); the reason is shown next to the
    /// manual token field.
    Failed(String),
}

/// Renew this many seconds before the stated expiry.
const RENEW_BEFORE_S: u64 = 30;

pub fn mint_url(gateway: &str) -> Option<String> {
    let g = gateway.trim().trim_end_matches('/');
    (!g.is_empty()).then(|| {
        let g = if g.starts_with("http://") || g.starts_with("https://") { g.to_owned() } else { format!("http://{g}") };
        format!("{g}/api/console/token")
    })
}

pub fn mint_body(project: &str) -> Vec<u8> {
    let p = project.trim();
    serde_json::json!({ "project": if p.is_empty() { serde_json::Value::Null } else { p.into() } }).to_string().into_bytes()
}

/// Read the gateway's answer (status + body) into a token or a reason.
pub fn parse_mint(status: u16, body: &[u8]) -> Result<Minted, String> {
    if status == 404 || status == 405 {
        return Err("this gateway does not issue console tokens".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("gateway refused a console token (HTTP {status})"));
    }
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("console token reply unreadable: {e}"))?;
    let token = v["token"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("console token reply has no token")?;
    Ok(Minted { token: token.to_owned(), expires_at: v["expires_at"].as_u64() })
}

impl Minted {
    pub fn expiring(&self, now: u64) -> bool {
        self.expires_at.is_some_and(|e| now + RENEW_BEFORE_S >= e)
    }
}

impl Client {
    /// The token the fleet reads use: the one typed in (or in the URL) wins, else the issued one.
    pub fn gateway_token(&self) -> String {
        let manual = self.s.gateway_token.trim();
        if !manual.is_empty() {
            return manual.to_owned();
        }
        match &*self.mint.lock().unwrap() {
            MintState::Done(m) => m.token.clone(),
            _ => String::new(),
        }
    }

    pub fn mint_state(&self) -> MintState {
        self.mint.lock().unwrap().clone()
    }

    /// Fire the mint when it is wanted and not already done, in flight or failed.
    pub(super) fn poll_mint(&mut self, ctx: &eframe::egui::Context) {
        let Some(url) = mint_url(&self.s.gateway) else { return };
        if !self.s.gateway_token.trim().is_empty() {
            return;
        }
        {
            let mut m = self.mint.lock().unwrap();
            match &*m {
                MintState::Idle => {}
                MintState::Done(t) if t.expiring(crate::views::fleet::now_unix()) => {}
                _ => return,
            }
            *m = MintState::Pending;
        }
        let mint = Arc::clone(&self.mint);
        let ctx = ctx.clone();
        let mut req = ehttp::Request::post(url, mint_body(&self.s.project));
        req.headers.insert("content-type", "application/json");
        ehttp::fetch(req, move |res| {
            let out = match &res {
                Ok(r) => parse_mint(r.status, &r.bytes),
                Err(e) => Err(format!("console token request failed: {e}")),
            };
            *mint.lock().unwrap() = match out {
                Ok(m) => MintState::Done(m),
                Err(e) => MintState::Failed(e),
            };
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_body() {
        assert_eq!(mint_url("10.0.0.1:8080/").as_deref(), Some("http://10.0.0.1:8080/api/console/token"));
        assert_eq!(mint_url("https://gw.example").as_deref(), Some("https://gw.example/api/console/token"));
        assert_eq!(mint_url(" "), None);
        assert_eq!(mint_body("01J8ZQ4W7K3M9N2P5R6T8V0XYZ"), br#"{"project":"01J8ZQ4W7K3M9N2P5R6T8V0XYZ"}"#);
        assert_eq!(mint_body(""), br#"{"project":null}"#);
    }

    #[test]
    fn reply_parsing() {
        let ok = parse_mint(200, br#"{"token":" abc ","expires_at":1900000000}"#).unwrap();
        assert_eq!(ok, Minted { token: "abc".into(), expires_at: Some(1_900_000_000) });
        assert_eq!(parse_mint(200, br#"{"token":"t"}"#).unwrap().expires_at, None);
        assert!(parse_mint(404, b"nope").unwrap_err().contains("does not issue"));
        assert!(parse_mint(403, b"").unwrap_err().contains("403"));
        assert!(parse_mint(200, br#"{"token":""}"#).is_err());
        assert!(parse_mint(200, b"<html>").is_err());
    }

    #[test]
    fn renews_shortly_before_expiry() {
        let m = Minted { token: "t".into(), expires_at: Some(1000) };
        assert!(!m.expiring(900));
        assert!(m.expiring(971));
        assert!(!Minted { token: "t".into(), expires_at: None }.expiring(u64::MAX / 2));
    }
}
