//! `oauth_uploader.php`: uploads files to Commons (or edits pages) under the
//! user's own account, via OAuth. With `botmode`, a JSON API: `checkauth`,
//! `upload`, `setpagetext`, `sdc`; without, a page to log in.

use crate::oauth::{OAuth, Session, Token};
use crate::public_web;
use crate::state::AppState;
use crate::web::{AppError, Params, is_same_origin, jsonp, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::PrivateCookieJar;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

const SESSION: Session = Session { name: "uploader_auth", path: "/oauth_uploader/" };
const MAX_FILE: u64 = 100 << 20;
/// A pause after each edit or upload, as the old tool made.
const EDIT_DELAY: Duration = Duration::from_secs(1);
/// Upload warnings refused with `blockwarnings=1`: the identical file is
/// already on Commons, or was deleted before.
const DEFAULT_BLOCKING_WARNINGS: &[&str] = &["duplicate", "duplicate-archive"];
const NOT_AUTHORIZED: &str =
    "You have not authorised this tool yet; log in at https://magnustools.toolforge.org/oauth_uploader/";

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    action: Option<String>,
    botmode: Option<String>,
    callback: Option<String>,
    site: Option<String>,
    #[serde(default)]
    page: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    newfile: String,
    #[serde(default)]
    desc: String,
    #[serde(default)]
    comment: String,
    ignorewarnings: Option<String>,
    blockwarnings: Option<String>,
    #[serde(default)]
    params: String,
    oauth_verifier: Option<String>,
    oauth_token: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/oauth_uploader.html")]
struct Page {
    title: &'static str,
    user: Option<String>,
    message: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let Some(oauth) = state.uploader_oauth.as_ref() else {
        return Err(AppError::Internal("the uploader's OAuth consumer is not configured".into()));
    };
    let jar = state.cookies(&headers);
    if let (Some(verifier), Some(token)) = (&request.oauth_verifier, &request.oauth_token) {
        return Ok(login_callback(oauth, jar, verifier, token).await);
    }
    let action = request.action.clone().unwrap_or_default();
    if action == "logout" {
        return Ok((SESSION.without_tokens(jar), Redirect::to(SESSION.path)).into_response());
    }
    if action == "authorize" {
        let (request_token, url) = oauth.initiate("oob").await?;
        return Ok((SESSION.with_request_token(jar, &request_token), Redirect::to(&url)).into_response());
    }
    let site = request.site.as_deref().unwrap_or("commons.wikimedia.org");
    let wiki = match Wiki::from_domain_prefix(site.trim().trim_end_matches(".org")) {
        Ok(wiki) => wiki,
        Err(e) if request.botmode.is_some() => {
            return Ok(jsonp(json!({"error": e.to_string(), "data": []}), None).into_response());
        }
        Err(e) => return Err(e.into()),
    };
    let user = User { oauth, token: SESSION.access_token(&jar), api: wiki.api_url() };

    let mut out = Map::from_iter([("error".to_string(), json!("OK")), ("data".to_string(), json!([]))]);
    let writes = ["setpagetext", "upload", "upload_from_url", "sdc"];
    let result: Result<(), String> = match action.as_str() {
        "checkauth" | "get_rights" => user.check().await.map(|rights| {
            let key = if action == "checkauth" { "data" } else { "result" };
            out.insert(key.to_string(), rights);
        }),
        // Changes only from this site: other sites could otherwise act in the user's name.
        a if writes.contains(&a) && !is_same_origin(&headers) => Err("Requests must come from this site".to_string()),
        "setpagetext" => user.edit_page(&request).await,
        "upload" | "upload_from_url" => {
            let (result, response) = user.upload(&request).await;
            out.insert("res".to_string(), response.unwrap_or(Value::Null));
            result
        }
        "sdc" => {
            let params: Value = serde_json::from_str(&request.params).unwrap_or(Value::Null);
            out.insert("jle".to_string(), json!(if params.is_null() { 4 } else { 0 }));
            out.insert("j".to_string(), params.clone());
            user.generic_action(params).await
        }
        _ if request.botmode.is_some() => Err("Unknown action".to_string()),
        _ => Ok(()),
    };

    if request.botmode.is_some() {
        if let Err(error) = result {
            out.insert("error".to_string(), error.into());
        }
        let callback = request.callback.map(|c| c.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect());
        return Ok(jsonp(Value::Object(out), callback).into_response());
    }
    let user_name = user.check().await.ok().and_then(|r| r["query"]["userinfo"]["name"].as_str().map(str::to_string));
    let message = match (action.as_str(), result) {
        ("checkauth", Ok(())) => Some("Auth OK!".to_string()),
        ("", Ok(())) => None,
        (_, Ok(())) => Some("Done.".to_string()),
        (_, Err(error)) => Some(error),
    };
    Ok(render(&Page { title: "OAuth uploader", user: user_name, message })?.into_response())
}

async fn login_callback(oauth: &OAuth, jar: PrivateCookieJar, verifier: &str, token: &str) -> Response {
    let Some(request) = SESSION.request_token(&jar).filter(|t| t.key == token) else {
        return Redirect::to(SESSION.path).into_response();
    };
    match oauth.access_token(&request, verifier).await {
        Ok(access) => (SESSION.with_access_token(jar, &access), Redirect::to(SESSION.path)).into_response(),
        Err(e) => {
            tracing::warn!("OAuth login failed: {e:#}");
            (StatusCode::BAD_GATEWAY, "The login failed; please try again.").into_response()
        }
    }
}

/// The logged-in user, acting on one wiki.
struct User<'a> {
    oauth: &'a OAuth,
    token: Option<Token>,
    api: String,
}

impl User<'_> {
    async fn api(&self, params: &[(&str, &str)]) -> Result<Value, String> {
        let token = self.token.as_ref().ok_or_else(|| NOT_AUTHORIZED.to_string())?;
        self.oauth.api(&self.api, Some(token), params).await.map_err(|e| format!("{e:#}"))
    }

    /// The user's rights, if they may use this tool: logged in and autoconfirmed.
    async fn check(&self) -> Result<Value, String> {
        let rights =
            self.api(&[("action", "query"), ("meta", "userinfo"), ("uiprop", "blockinfo|groups|rights")]).await?;
        if rights["error"]["code"] == "mwoauth-invalid-authorization" {
            return Err(NOT_AUTHORIZED.to_string());
        }
        let userinfo = &rights["query"]["userinfo"];
        if userinfo.is_null() || userinfo.get("anon").is_some() {
            return Err("Auth not OK: not logged in".to_string());
        }
        let groups = userinfo["groups"].as_array().into_iter().flatten();
        if !groups.filter_map(Value::as_str).any(|g| g == "autoconfirmed") {
            return Err("Auth not OK: your account needs to be autoconfirmed".to_string());
        }
        Ok(rights)
    }

    async fn csrf_token(&self) -> Result<String, String> {
        let answer = self.api(&[("action", "query"), ("meta", "tokens")]).await?;
        answer["query"]["tokens"]["csrftoken"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("Bad API response: {answer}"))
    }

    async fn edit_page(&self, request: &Request) -> Result<(), String> {
        self.check().await?;
        let text = request.text.trim();
        if text.is_empty() {
            return Err("No text given".to_string());
        }
        let token = self.csrf_token().await?;
        let answer = self
            .api(&[
                ("action", "edit"),
                ("title", request.page.trim()),
                ("text", text),
                ("minor", ""),
                ("token", &token),
            ])
            .await?;
        api_error(&answer).map_or(Ok(()), Err)?;
        tokio::time::sleep(EDIT_DELAY).await;
        Ok(())
    }

    /// Any API action given as JSON, e.g. `wbeditentity` for structured data.
    async fn generic_action(&self, params: Value) -> Result<(), String> {
        self.check().await?;
        let Value::Object(params) = params else { return Err("params must be a JSON object".to_string()) };
        if !params.contains_key("action") {
            return Err(format!("No action in {}", Value::Object(params)));
        }
        let token = self.csrf_token().await?;
        let mut fields: Vec<(String, String)> = params
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    match v {
                        Value::String(s) => s,
                        other => other.to_string(),
                    },
                )
            })
            .collect();
        fields.extend([("token".to_string(), token), ("bot".to_string(), "1".to_string())]);
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let answer = self.api(&fields).await?;
        api_error(&answer).map_or(Ok(()), Err)?;
        tokio::time::sleep(EDIT_DELAY).await;
        Ok(())
    }

    /// Downloads the file from `url` and uploads it. Also returns the API's
    /// last answer, which callers read (`res.upload.filename`).
    async fn upload(&self, request: &Request) -> (Result<(), String>, Option<Value>) {
        let mut last = None;
        let result = self.try_upload(request, &mut last).await;
        (result, last)
    }

    async fn try_upload(&self, request: &Request, last: &mut Option<Value>) -> Result<(), String> {
        self.check().await?;
        let url = request.url.trim();
        if url.is_empty() {
            return Err("No URL given".to_string());
        }
        let blocking = blocking_warnings(request.blockwarnings.as_deref());
        let ignore_warnings = request.ignorewarnings.is_some();
        let filename = upload_name(&request.newfile, url);

        let (file, size) = public_web::download(url, MAX_FILE)
            .await
            .map_err(|e| format!("Could not download the source URL: {e:#} ({url})"))?;
        let token = self.csrf_token().await?;
        let mut fields = vec![
            ("action", "upload".to_string()),
            ("comment", request.comment.trim().to_string()),
            ("text", request.desc.trim().to_string()),
            ("token", token),
            ("filename", filename.clone()),
        ];
        // When warnings are inspected, the first request must not suppress them.
        if ignore_warnings && blocking.is_empty() {
            fields.push(("ignorewarnings", "1".to_string()));
        }
        let reader = tokio::fs::File::open(file.path()).await.map_err(|e| e.to_string())?;
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(reader));
        let part = reqwest::multipart::Part::stream_with_length(body, size)
            .file_name(filename)
            .mime_str("application/octet-stream")
            .map_err(|e| e.to_string())?;
        let access = self.token.as_ref().ok_or_else(|| NOT_AUTHORIZED.to_string())?;
        let mut answer = self.oauth.api_upload(&self.api, access, &fields, part).await.map_err(|e| format!("{e:#}"))?;
        *last = Some(answer.clone());

        let upload = &answer["upload"];
        if !blocking.is_empty() && upload["result"] == "Warning" {
            let warnings = upload["warnings"].as_object().cloned().unwrap_or_default();
            let blocked = blocking.iter().any(|key| warnings.get(key).is_some_and(|v| !is_empty(v)));
            if blocked {
                // The file stays in the upload stash, which expires by itself.
                return Err(describe_upload(upload));
            }
            if let (true, Some(filekey)) = (ignore_warnings, upload["filekey"].as_str()) {
                // Nothing the caller cares about: publish the stashed file.
                let filekey = filekey.to_string();
                let mut publish: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
                publish.extend([("filekey", filekey.as_str()), ("ignorewarnings", "1")]);
                answer = self.api(&publish).await?;
                *last = Some(answer.clone());
            }
        }
        let upload = &answer["upload"];
        if upload.is_null() {
            return Err(api_error(&answer).unwrap_or_else(|| {
                "No valid response from the API (the upload may or may not have happened)".to_string()
            }));
        }
        if upload["result"] != "Success" {
            return Err(describe_upload(upload));
        }
        tokio::time::sleep(EDIT_DELAY).await;
        Ok(())
    }
}

/// The file name on the wiki: as given, or the URL's last part; spaces as underscores, first letter upper case.
fn upload_name(newfile: &str, url: &str) -> String {
    let name = match newfile.trim() {
        "" => url.rsplit('/').next().unwrap_or_default(),
        name => name,
    };
    let name = name.replace(' ', "_");
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// From `blockwarnings`: absent, empty, `0` or `false` blocks nothing; `1` or
/// `true` the defaults; otherwise a `|`- or `,`-separated list of warning keys.
fn blocking_warnings(raw: Option<&str>) -> Vec<String> {
    let raw = raw.unwrap_or_default().trim();
    match raw.to_lowercase().as_str() {
        "" | "0" | "false" => vec![],
        "1" | "true" => DEFAULT_BLOCKING_WARNINGS.iter().map(|w| w.to_string()).collect(),
        _ => {
            let mut keys: Vec<String> = raw
                .split(['|', ','])
                .map(str::trim)
                .filter(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                .map(str::to_string)
                .collect();
            keys.dedup();
            keys
        }
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// `info [code]` of an API error, if the answer is one.
fn api_error(answer: &Value) -> Option<String> {
    let error = answer.get("error")?;
    let (info, code) =
        (error["info"].as_str().unwrap_or_default().trim(), error["code"].as_str().unwrap_or_default().trim());
    Some(match (info.is_empty(), code.is_empty()) {
        (false, false) => format!("{info} [{code}]"),
        (false, true) => info.to_string(),
        (true, false) => code.to_string(),
        (true, true) => "Unspecified API error".to_string(),
    })
}

/// An unsuccessful upload in words a user can act on, not just "Warning".
fn describe_upload(upload: &Value) -> String {
    let result = upload["result"].as_str().unwrap_or("Unknown");
    let Some(warnings) = upload["warnings"].as_object() else { return result.to_string() };
    let text = |key: &str| warnings.get(key).map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string));
    let mut parts = vec![];
    if let Some(first) = warnings.get("duplicate").and_then(Value::as_array).and_then(|d| d.first()) {
        parts.push(format!("identical file already on Commons as \"{}\"", first.as_str().unwrap_or_default()));
    }
    let described: &[(&str, &str)] = &[
        ("exists", "a different file already uses the name"),
        ("exists-normalized", "a file with an equivalent name exists:"),
        ("duplicate-archive", "an identical file was uploaded and deleted before:"),
        ("was-deleted", "this filename was deleted before:"),
        ("badfilename", "Commons would rename the file to"),
    ];
    for (key, words) in described {
        if let Some(value) = text(key) {
            parts.push(format!("{words} \"{value}\""));
        }
    }
    let flags: &[(&[&str], &str)] = &[
        (&["emptyfile"], "the downloaded file was empty"),
        (&["filetype-unwanted-type", "filetype-banned"], "this file type is not allowed on Commons"),
        (&["large-file"], "the file is larger than Commons allows"),
        (&["was-deleted-archive"], "an earlier version of this file was deleted"),
    ];
    for (keys, words) in flags {
        if keys.iter().any(|k| warnings.contains_key(*k)) {
            parts.push(words.to_string());
        }
    }
    // Anything else MediaWiki warned about is still worth showing.
    let known: Vec<&str> = described
        .iter()
        .map(|(k, _)| *k)
        .chain(flags.iter().flat_map(|(keys, _)| keys.iter().copied()))
        .chain(["duplicate"])
        .collect();
    for (key, value) in warnings.iter().filter(|(k, _)| !known.contains(&k.as_str())) {
        match value {
            Value::String(s) if s.is_empty() => parts.push(key.clone()),
            Value::Bool(true) => parts.push(key.clone()),
            other => parts.push(format!("{key}: {other}")),
        }
    }
    if parts.is_empty() { result.to_string() } else { format!("{result}: {}", parts.join("; ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(upload_name("", "https://x.org/a/my photo.jpg"), "My_photo.jpg");
        assert_eq!(upload_name(" été 2024.png ", "https://x.org/y"), "Été_2024.png");
    }

    #[test]
    fn warning_lists() {
        assert!(blocking_warnings(None).is_empty());
        assert!(blocking_warnings(Some("false")).is_empty());
        assert_eq!(blocking_warnings(Some("1")), vec!["duplicate", "duplicate-archive"]);
        assert_eq!(blocking_warnings(Some("exists|bad key,was-deleted")), vec!["exists", "was-deleted"]);
    }

    #[test]
    fn upload_descriptions() {
        let upload = json!({"result": "Warning", "warnings": {"duplicate": ["Other.jpg"], "exists": "Name.jpg", "nochange": ""}});
        assert_eq!(
            describe_upload(&upload),
            "Warning: identical file already on Commons as \"Other.jpg\"; a different file already uses the name \"Name.jpg\"; nochange"
        );
        assert_eq!(describe_upload(&json!({"result": "Success"})), "Success");
        assert_eq!(
            api_error(&json!({"error": {"code": "badtoken", "info": "Invalid CSRF token."}})).as_deref(),
            Some("Invalid CSRF token. [badtoken]")
        );
        assert_eq!(api_error(&json!({"upload": {}})), None);
    }
}
