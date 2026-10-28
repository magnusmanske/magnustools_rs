//! `buggregator/api.php`: the JSON API of the buggregator front-end, and its
//! OAuth login. Answers are `{"status": "OK" | <error>, "data": …}`.

use super::dashboard::{self, admin, query};
use super::issues::{self, IssueQuery, PRIORITIES, SITES, SORT_BY, STATUSES};
use super::usage;
use crate::oauth::{OAuth, Session, Token};
use crate::state::AppState;
use crate::web::{Params, is_same_origin};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Json, Redirect, Response};
use axum_extra::extract::PrivateCookieJar;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Who may edit issues and acknowledge alerts.
const WRITE_ACCESS: &[&str] = &["Magnus Manske"];
const SESSION: Session = Session { name: "buggregator_auth", path: "/buggregator" };
/// The consumer is registered on mediawiki.org.
const API_URL: &str = "https://www.mediawiki.org/w/api.php";
const NOT_ALLOWED: &str = "User not logged in, or not in whitelist";

pub async fn handler(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    Params(params): Params<HashMap<String, String>>,
) -> Response {
    let jar = state.cookies(&headers);
    let get = |key: &str| params.get(key).cloned();
    let Some(oauth) = state.buggregator_oauth.as_ref() else {
        return answer(json!({"status": "Login is not configured"}));
    };
    if let (Some(verifier), Some(token)) = (get("oauth_verifier"), get("oauth_token")) {
        return login_callback(oauth, jar, &verifier, &token, &state.config.public_url).await;
    }
    let request = Request { state: &state, oauth, jar: &jar, headers: &headers, method: &method, get: &get };
    match request.answer(get("action").unwrap_or_default().as_str()).await {
        Ok(Reply::Json(out)) => answer(Value::Object(out)),
        Ok(Reply::Redirect(url, jar)) => (jar, Redirect::to(&url)).into_response(),
        Err(e) => {
            tracing::warn!("buggregator API: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"status": "Internal error", "data": []}))).into_response()
        }
    }
}

fn answer(out: Value) -> Response {
    Json(out).into_response()
}

enum Reply {
    Json(Map<String, Value>),
    Redirect(String, PrivateCookieJar),
}

struct Request<'a, F: Fn(&str) -> Option<String>> {
    state: &'a AppState,
    oauth: &'a OAuth,
    jar: &'a PrivateCookieJar,
    headers: &'a HeaderMap,
    method: &'a Method,
    get: &'a F,
}

impl<F: Fn(&str) -> Option<String>> Request<'_, F> {
    async fn answer(&self, action: &str) -> anyhow::Result<Reply> {
        let mut out = Map::from_iter([("status".to_string(), json!("OK")), ("data".to_string(), json!([]))]);
        let text = |key: &str| (self.get)(key).unwrap_or_default();
        let number = |key: &str| text(key).trim().parse::<u64>().unwrap_or_default();
        let now = dashboard::now();
        match action {
            "authorize" => {
                let callback = format!("{}/buggregator/api.php", self.state.config.public_url);
                let (request_token, url) = self.oauth.initiate(&callback).await?;
                let jar = SESSION.with_request_token(self.jar.clone(), &request_token);
                return Ok(Reply::Redirect(url, jar));
            }
            "logout" => {
                let jar = SESSION.without_tokens(self.jar.clone());
                return Ok(Reply::Redirect(format!("{}/buggregator/", self.state.config.public_url), jar));
            }
            "get_rights" => {
                out.insert("result".to_string(), self.rights().await);
            }
            "get_issues" => {
                let mut conn = self.state.tool_db.conn().await?;
                out.insert("data".to_string(), issues::get_issues(&mut conn, &IssueQuery::new(self.get)).await?);
            }
            "get_config" => {
                let mut conn = self.state.tool_db.conn().await?;
                out.insert(
                    "data".to_string(),
                    json!({
                        "status": STATUSES, "site": SITES, "priority": PRIORITIES,
                        "write_access": WRITE_ACCESS, "sort_by": SORT_BY,
                        "tools": issues::tools_by_id(&mut conn).await?,
                    }),
                );
            }
            "get_tools" => {
                let mut conn = self.state.tool_db.conn().await?;
                out.insert("data".to_string(), issues::get_tools(&mut conn).await?);
            }
            "set_issue_status" | "set_issue_tool" => {
                let (issue_id, status) = (number("issue_id"), text("new_status"));
                let status = match self.editor().await? {
                    None => NOT_ALLOWED.to_string(),
                    Some(_) if issue_id == 0 => "No issue ID set".to_string(),
                    Some(_) if action == "set_issue_status" && !STATUSES.contains(&status.as_str()) => {
                        format!("'{status}' is not a valid issue status")
                    }
                    Some(user) => {
                        let mut conn = self.state.tool_db.conn().await?;
                        match action {
                            "set_issue_status" => issues::set_status(&mut conn, issue_id, &status, &user).await?,
                            _ => issues::set_tool(&mut conn, issue_id, number("new_tool_id"), &user).await?,
                        }
                        out.insert("user".to_string(), user.into());
                        "OK".to_string()
                    }
                };
                out.insert("status".to_string(), status.into());
            }
            "dashboard" => {
                let mut conn = self.state.tool_db.conn().await?;
                out.insert("data".to_string(), json!({"services": query::overview(&mut conn, now).await?, "now": now}));
            }
            "dashboard_totals" => {
                let mut conn = self.state.tool_db.conn().await?;
                out.insert("data".to_string(), query::totals(&mut conn, query::range(&text("range")), now).await?);
            }
            "dashboard_series" | "dashboard_instances" => {
                let mut conn = self.state.tool_db.conn().await?;
                let (service, component, range) = (text("service"), text("component"), query::range(&text("range")));
                let data = match action {
                    "dashboard_series" => query::series(&mut conn, &service, &component, range, now).await?,
                    _ => query::instances(&mut conn, &service, &component, range, now).await?,
                };
                out.insert("data".to_string(), data);
            }
            // Event messages can contain internals, so they are not public.
            "dashboard_events" => match self.editor().await? {
                None => {
                    out.insert("status".to_string(), NOT_ALLOWED.into());
                }
                Some(_) => {
                    let mut conn = self.state.tool_db.conn().await?;
                    let events = query::events(&mut conn, &text("service"), query::range(&text("range")), now).await?;
                    out.insert("data".to_string(), events);
                }
            },
            "dashboard_ack" => {
                let status = match (*self.method == Method::POST, self.editor().await?) {
                    (false, _) => "POST required".to_string(),
                    (true, None) => NOT_ALLOWED.to_string(),
                    (true, Some(user)) => {
                        let mut conn = self.state.tool_db.conn().await?;
                        let (service, component, kind) = (text("service"), text("component"), text("kind"));
                        let ack = admin::acknowledge(&mut conn, &service, &component, &kind, &user, now).await?;
                        out.insert("user".to_string(), user.into());
                        ack.err().unwrap_or_else(|| "OK".to_string())
                    }
                };
                out.insert("status".to_string(), status.into());
            }
            "usage" | "usage_tool" => {
                let mut conn = self.state.tool_db.conn().await?;
                // Complete days only: today's count is still growing.
                let today = chrono::Utc::now().date_naive();
                let (days, last) = (usage::range(&text("range")), today.pred_opt().unwrap_or(today));
                let data = match action {
                    "usage" => usage::overview(&mut conn, days, last).await?,
                    _ => usage::tool(&mut conn, &text("tool"), days, last).await?,
                };
                out.insert("data".to_string(), data);
            }
            _ => {
                out.insert("status".to_string(), format!("Unknown action '{action}'").into());
            }
        }
        Ok(Reply::Json(out))
    }

    /// The user's rights as MediaWiki reports them (`query.userinfo`), which
    /// the front-end's login widget reads; an error object when logged out.
    async fn rights(&self) -> Value {
        let Some(token) = SESSION.access_token(self.jar) else {
            return json!({"error": {"code": "notloggedin", "info": "Not logged in"}});
        };
        let params = [("action", "query"), ("meta", "userinfo"), ("uiprop", "blockinfo|groups|rights")];
        self.oauth.api(API_URL, Some(&token), &params).await.unwrap_or_else(|e| {
            tracing::warn!("userinfo: {e:#}");
            json!({"error": {"code": "unavailable", "info": "Could not reach the wiki"}})
        })
    }

    /// The logged-in user, if they may edit. Changes must come from the
    /// buggregator page itself (the front-end sends them as GET requests,
    /// so a POST cannot be required).
    async fn editor(&self) -> anyhow::Result<Option<String>> {
        if !is_same_origin(self.headers) {
            return Ok(None);
        }
        let rights = self.rights().await;
        let name = rights["query"]["userinfo"]["name"].as_str().unwrap_or_default();
        Ok(WRITE_ACCESS.contains(&name).then(|| name.to_string()))
    }
}

/// MediaWiki sends the user back with the approved request token.
async fn login_callback(
    oauth: &OAuth,
    jar: PrivateCookieJar,
    verifier: &str,
    token: &str,
    public_url: &str,
) -> Response {
    let home = format!("{public_url}/buggregator/");
    let request: Option<Token> = SESSION.request_token(&jar).filter(|t| t.key == token);
    let Some(request) = request else {
        // No login in progress (e.g. a revisited callback URL).
        return Redirect::to(&home).into_response();
    };
    match oauth.access_token(&request, verifier).await {
        Ok(access) => (SESSION.with_access_token(jar, &access), Redirect::to(&home)).into_response(),
        Err(e) => {
            tracing::warn!("OAuth login failed: {e:#}");
            (StatusCode::BAD_GATEWAY, "The login failed; please try again.").into_response()
        }
    }
}
