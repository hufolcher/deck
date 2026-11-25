//! HTTP through curl, typed both ways: a serializable body in, a deserialized
//! answer out. Tokens come from the keychain and go to curl on stdin.

use super::Error;
use super::shell::{Invocation, Shell};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A secret. Its Debug never prints it.
#[derive(Clone, PartialEq)]
pub struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("Token(…)")
    }
}

/// How an API wants its token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scheme {
    /// `Authorization: <token>` (Linear personal keys).
    Raw,
    /// `Authorization: Bearer <token>` (Slack).
    Bearer,
}

impl Token {
    /// None for an empty one.
    pub fn new(value: &str) -> Option<Token> {
        let value = value.trim();
        (!value.is_empty()).then(|| Token(value.into()))
    }

    fn header(&self, scheme: Scheme) -> String {
        match scheme {
            Scheme::Raw => format!("Authorization: {}", self.0),
            Scheme::Bearer => format!("Authorization: Bearer {}", self.0),
        }
    }
}

/// A connector's token: `DECK_<NAME>_TOKEN`, else the keychain item
/// `deck-<name>` (macOS `security`, elsewhere `secret-tool`).
pub fn token<S: Shell>(shell: &S, name: &'static str) -> Result<Token, Error> {
    let variable = format!("DECK_{}_TOKEN", name.to_uppercase());
    if let Some(value) = std::env::var(variable)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(Token(value));
    }
    let service = format!("deck-{name}");
    let lookup = if cfg!(target_os = "macos") {
        Invocation::new("security").args(["find-generic-password", "-s", &service, "-w"])
    } else {
        Invocation::new("secret-tool").args(["lookup", "service", &service])
    };
    shell
        .run(&lookup)
        .ok()
        .filter(|output| output.success)
        .map(|output| output.stdout.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Token)
        .ok_or(Error::NotConnected(name))
}

pub struct Http<S: Shell> {
    shell: S,
}

impl<S: Shell> Http<S> {
    pub fn new(shell: S) -> Http<S> {
        Http { shell }
    }

    pub fn get<T: DeserializeOwned>(
        &self,
        url: &str,
        token: &Token,
        scheme: Scheme,
    ) -> Result<T, Error> {
        self.send(Invocation::new("curl"), url, token.header(scheme))
    }

    /// A form POST, as Slack's Web API takes its arguments.
    pub fn form<T: DeserializeOwned>(
        &self,
        url: &str,
        token: &Token,
        scheme: Scheme,
        fields: &[(&str, &str)],
    ) -> Result<T, Error> {
        let mut curl = Invocation::new("curl").args(["-X", "POST"]);
        for (name, value) in fields {
            curl = curl.args(["--data-urlencode", &format!("{name}={value}")]);
        }
        self.send(curl, url, token.header(scheme))
    }

    pub fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        url: &str,
        token: &Token,
        scheme: Scheme,
        body: &B,
    ) -> Result<T, Error> {
        let body = serde_json::to_string(body).map_err(|error| Error::Decode(error.to_string()))?;
        let curl = Invocation::new("curl").args(["--data-binary", &body]);
        self.send(
            curl,
            url,
            format!("{}\nContent-Type: application/json", token.header(scheme)),
        )
    }

    fn send<T: DeserializeOwned>(
        &self,
        curl: Invocation,
        url: &str,
        headers: String,
    ) -> Result<T, Error> {
        let invocation = curl
            .args(["-sS", "--max-time", "20", "-H", "@-"])
            .arg(url)
            .stdin(headers);
        let output = self
            .shell
            .run(&invocation)
            .map_err(|error| Error::Program("curl", error.to_string()))?;
        if !output.success {
            return Err(Error::Program("curl", output.reason()));
        }
        serde_json::from_str(&output.stdout).map_err(|error| Error::Decode(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::shell::fake::Fake;
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct Pong {
        pong: u8,
    }

    #[test]
    fn token_on_stdin_never_in_args() {
        let fake = Fake::default().answer("curl", r#"{"pong": 1}"#);
        let http = Http::new(&fake);
        let answer: Pong = http
            .post(
                "https://api.test/x",
                &Token("s3cret".into()),
                Scheme::Bearer,
                &["ping"],
            )
            .unwrap();
        assert_eq!(answer, Pong { pong: 1 });
        let call = &fake.calls.borrow()[0];
        assert!(!call.line().contains("s3cret"));
        assert_eq!(
            call.stdin.as_deref(),
            Some("Authorization: Bearer s3cret\nContent-Type: application/json")
        );
        assert!(call.line().contains(r#"--data-binary ["ping"]"#));
    }

    #[test]
    fn form_fields_urlencoded_token_on_stdin() {
        let fake = Fake::default().answer("curl", r#"{"pong": 2}"#);
        let answer: Pong = Http::new(&fake)
            .form(
                "https://api.test/m",
                &Token("s3cret".into()),
                Scheme::Bearer,
                &[("channel", "C1"), ("ts", "1.2")],
            )
            .unwrap();
        assert_eq!(answer, Pong { pong: 2 });
        let call = &fake.calls.borrow()[0];
        assert!(
            call.line()
                .contains("-X POST --data-urlencode channel=C1 --data-urlencode ts=1.2")
        );
        assert!(!call.line().contains("s3cret"));
    }

    #[test]
    fn curl_failure_and_bad_json_are_typed() {
        let failing = Fake::default().fail("curl", "curl: (6) Could not resolve host");
        let error = Http::new(&failing)
            .get::<Pong>("https://api.test", &Token("t".into()), Scheme::Raw)
            .unwrap_err();
        assert_eq!(
            error,
            Error::Program("curl", "curl: (6) Could not resolve host".into())
        );
        let garbled = Fake::default().answer("curl", "<html>");
        let error = Http::new(&garbled)
            .get::<Pong>("https://api.test", &Token("t".into()), Scheme::Raw)
            .unwrap_err();
        assert!(matches!(error, Error::Decode(_)));
    }

    #[test]
    fn keychain_then_not_connected() {
        let keychain = Fake::default()
            .answer("deck-linear", "lin_api_x\n")
            .answer("lookup service deck-linear", "lin_api_x\n");
        assert_eq!(token(&keychain, "linear"), Ok(Token("lin_api_x".into())));
        assert_eq!(
            token(&Fake::default(), "linear"),
            Err(Error::NotConnected("linear"))
        );
        assert_eq!(format!("{:?}", Token("x".into())), "Token(…)");
    }
}
