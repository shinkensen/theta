use reqwest::{header::HeaderMap, Client, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

const AUTH_FILE: &str = "canvas_auth.json";
const MAX_PAGES: usize = 50;
const MAX_ITEMS: usize = 5_000;
const MAX_DUE_COURSES: usize = 25;
const DEFAULT_RESULT_LIMIT: usize = 50;
const MAX_RESULT_LIMIT: usize = 100;

#[derive(Clone, Serialize, Deserialize)]
struct CanvasAuth {
    base_url: Url,
    token: String,
}

pub struct CanvasState {
    auth_path: PathBuf,
    auth: Mutex<Option<CanvasAuth>>,
    http: Client,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasStatus {
    pub connected: bool,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasProfile {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub short_name: String,
    #[serde(default)]
    pub sortable_name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasCourse {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub course_code: String,
    #[serde(default)]
    pub workflow_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasAssignment {
    pub id: u64,
    pub course_id: u64,
    pub name: String,
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub points_possible: Option<f64>,
    #[serde(default)]
    pub submission: Option<CanvasSubmission>,
    #[serde(default)]
    pub all_dates: Vec<CanvasAssignmentDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasSubmission {
    #[serde(default)]
    pub workflow_state: Option<String>,
    #[serde(default)]
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub excused: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasAssignmentDate {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub base: Option<bool>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub due_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasDueDate {
    pub assignment_id: u64,
    pub course_id: u64,
    pub course_name: String,
    pub name: String,
    pub due_at: String,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasModule {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub position: u64,
    #[serde(default)]
    pub unlock_at: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasModuleItem {
    pub id: u64,
    pub title: String,
    #[serde(rename(deserialize = "type"), default)]
    pub item_type: String,
    #[serde(default)]
    pub position: u64,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub content_id: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub completion_requirement: Option<CanvasCompletionRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct CanvasCompletionRequirement {
    #[serde(rename = "type", default)]
    pub requirement_type: String,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub min_score: Option<f64>,
}

fn result_limit(max_items: Option<usize>) -> usize {
    max_items
        .unwrap_or(DEFAULT_RESULT_LIMIT)
        .clamp(1, MAX_RESULT_LIMIT)
}

fn cap<T>(items: &mut Vec<T>, max_items: Option<usize>) {
    items.truncate(result_limit(max_items));
}

fn submission_complete(assignment: &CanvasAssignment) -> bool {
    assignment.submission.as_ref().is_some_and(|submission| {
        submission.excused.unwrap_or(false)
            || submission.submitted_at.is_some()
            || matches!(
                submission.workflow_state.as_deref(),
                Some("submitted" | "graded")
            )
    })
}

impl CanvasState {
    pub fn load(data_dir: &Path) -> Self {
        let auth_path = data_dir.join(AUTH_FILE);
        let auth = fs::read(&auth_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CanvasAuth>(&bytes).ok())
            .filter(|a| {
                validate_root(a.base_url.as_str()).is_ok() && validate_token(&a.token).is_ok()
            });
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .user_agent("Theta Canvas integration")
            .build()
            .expect("Canvas HTTP client configuration is valid");
        Self {
            auth_path,
            auth: Mutex::new(auth),
            http,
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<CanvasAuth>>, String> {
        self.auth
            .lock()
            .map_err(|_| "Canvas configuration is unavailable".into())
    }

    fn configured(&self) -> Result<CanvasAuth, String> {
        self.lock()?
            .clone()
            .ok_or_else(|| "Canvas is not connected".into())
    }
}

fn validate_root(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(value.trim()).map_err(|_| "Canvas URL is invalid".to_string())?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("Canvas URL must be an HTTPS origin".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Canvas URL cannot contain credentials, a query, or a fragment".into());
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err("Canvas URL must not contain a path".into());
    }
    url.set_path("/");
    Ok(url)
}

fn validate_token(token: &str) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() {
        Err("Canvas token cannot be empty".into())
    } else if token.chars().any(char::is_whitespace) {
        Err("Canvas token cannot contain whitespace".into())
    } else {
        Ok(token.to_owned())
    }
}

fn endpoint(base: &Url, path: &str) -> Result<Url, String> {
    base.join(path)
        .map_err(|_| "Invalid Canvas API endpoint".into())
}

fn safe_status(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            "Canvas rejected the credentials".into()
        }
        StatusCode::NOT_FOUND => "Canvas resource was not found".into(),
        StatusCode::TOO_MANY_REQUESTS => "Canvas rate limit exceeded".into(),
        _ if status.is_server_error() => "Canvas is temporarily unavailable".into(),
        _ => format!("Canvas request failed with status {}", status.as_u16()),
    }
}

fn next_link(headers: &HeaderMap, current: &Url, root: &Url) -> Result<Option<Url>, String> {
    let Some(raw) = headers.get(reqwest::header::LINK) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| "Canvas returned an invalid pagination link")?;
    for part in raw.split(',') {
        let mut pieces = part.split(';');
        let target = pieces.next().unwrap_or("").trim();
        let is_next = pieces.any(|p| {
            p.trim().split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("rel")
                    && v.trim()
                        .trim_matches('"')
                        .split_whitespace()
                        .any(|r| r == "next")
            })
        });
        if !is_next {
            continue;
        }
        let href = target
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .ok_or_else(|| "Canvas returned an invalid pagination link".to_string())?;
        let next = current
            .join(href)
            .map_err(|_| "Canvas returned an invalid pagination link")?;
        if next.scheme() != "https"
            || next.origin() != root.origin()
            || !next.username().is_empty()
            || next.password().is_some()
            || next.fragment().is_some()
        {
            return Err("Canvas pagination left the configured HTTPS origin".into());
        }
        return Ok(Some(next));
    }
    Ok(None)
}

async fn get_one<T: DeserializeOwned>(
    state: &CanvasState,
    auth: &CanvasAuth,
    url: Url,
) -> Result<T, String> {
    let response = state
        .http
        .get(url)
        .bearer_auth(&auth.token)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Canvas request timed out".to_string()
            } else {
                "Could not reach Canvas".to_string()
            }
        })?;
    if !response.status().is_success() {
        return Err(safe_status(response.status()));
    }
    response
        .json()
        .await
        .map_err(|_| "Canvas returned invalid data".into())
}

async fn get_pages<T: DeserializeOwned>(
    state: &CanvasState,
    auth: &CanvasAuth,
    first: Url,
) -> Result<Vec<T>, String> {
    let mut url = first;
    let mut visited = HashSet::new();
    let mut all = Vec::new();
    for _ in 0..MAX_PAGES {
        if !visited.insert(url.as_str().to_owned()) {
            return Err("Canvas pagination loop detected".into());
        }
        let response = state
            .http
            .get(url.clone())
            .bearer_auth(&auth.token)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "Canvas request timed out".to_string()
                } else {
                    "Could not reach Canvas".to_string()
                }
            })?;
        if !response.status().is_success() {
            return Err(safe_status(response.status()));
        }
        let next = next_link(response.headers(), &url, &auth.base_url)?;
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Could not read the Canvas response".to_string())?;
        let page: Vec<T> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Canvas returned invalid data: {error}"))?;
        if all.len().saturating_add(page.len()) > MAX_ITEMS {
            return Err("Canvas response was truncated at the safety limit".into());
        }
        all.extend(page);
        match next {
            Some(value) => url = value,
            None => return Ok(all),
        }
    }
    Err("Canvas response was truncated at the page limit".into())
}

#[tauri::command]
pub fn canvas_status(state: tauri::State<'_, CanvasState>) -> Result<CanvasStatus, String> {
    let auth = state.lock()?;
    Ok(CanvasStatus {
        connected: auth.is_some(),
        base_url: auth
            .as_ref()
            .map(|a| a.base_url.origin().ascii_serialization()),
    })
}

#[tauri::command]
pub async fn canvas_save_config(
    base_url: String,
    token: String,
    state: tauri::State<'_, CanvasState>,
) -> Result<CanvasProfile, String> {
    let candidate = CanvasAuth {
        base_url: validate_root(&base_url)?,
        token: validate_token(&token)?,
    };
    let profile = get_one(
        &state,
        &candidate,
        endpoint(&candidate.base_url, "api/v1/users/self/profile")?,
    )
    .await?;
    let bytes =
        serde_json::to_vec(&candidate).map_err(|_| "Could not encode Canvas configuration")?;
    if let Some(parent) = state.auth_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "Could not create Canvas configuration directory")?;
    }
    let temp = state.auth_path.with_extension("json.tmp");
    fs::write(&temp, bytes).map_err(|_| "Could not save Canvas configuration")?;
    if state.auth_path.exists() {
        fs::remove_file(&state.auth_path).map_err(|_| "Could not replace Canvas configuration")?;
    }
    fs::rename(&temp, &state.auth_path).map_err(|_| "Could not save Canvas configuration")?;
    *state.lock()? = Some(candidate);
    Ok(profile)
}

#[tauri::command]
pub fn canvas_disconnect(state: tauri::State<'_, CanvasState>) -> Result<(), String> {
    match fs::remove_file(&state.auth_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("Could not remove Canvas configuration".into()),
    }
    *state.lock()? = None;
    Ok(())
}

#[tauri::command]
pub async fn canvas_get_profile(
    state: tauri::State<'_, CanvasState>,
) -> Result<CanvasProfile, String> {
    let auth = state.configured()?;
    get_one(
        &state,
        &auth,
        endpoint(&auth.base_url, "api/v1/users/self/profile")?,
    )
    .await
}

async fn active_courses(
    state: &CanvasState,
    auth: &CanvasAuth,
) -> Result<Vec<CanvasCourse>, String> {
    let mut url = endpoint(&auth.base_url, "api/v1/courses")?;
    url.query_pairs_mut()
        .append_pair("enrollment_state", "active")
        .append_pair("per_page", "100");
    let mut courses = get_pages::<CanvasCourse>(state, auth, url).await?;
    courses.retain(|c| c.workflow_state != "deleted" && !c.name.trim().is_empty());
    courses.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    courses.dedup_by_key(|c| c.id);
    Ok(courses)
}

#[tauri::command]
pub async fn canvas_list_active_courses(
    max_items: Option<usize>,
    state: tauri::State<'_, CanvasState>,
) -> Result<Vec<CanvasCourse>, String> {
    let auth = state.configured()?;
    let mut courses = active_courses(&state, &auth).await?;
    cap(&mut courses, max_items);
    Ok(courses)
}

async fn assignments(
    state: &CanvasState,
    auth: &CanvasAuth,
    course_id: u64,
) -> Result<Vec<CanvasAssignment>, String> {
    let mut url = endpoint(
        &auth.base_url,
        &format!("api/v1/courses/{course_id}/assignments"),
    )?;
    url.query_pairs_mut()
        .append_pair("per_page", "100")
        .append_pair("include[]", "submission")
        .append_pair("include[]", "all_dates");
    let mut items = get_pages::<CanvasAssignment>(state, auth, url).await?;
    items.retain(|a| !a.name.trim().is_empty());
    items.sort_by(|a, b| {
        a.due_at
            .cmp(&b.due_at)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then(a.id.cmp(&b.id))
    });
    items.dedup_by_key(|a| a.id);
    Ok(items)
}

#[tauri::command]
pub async fn canvas_list_assignments(
    course_id: u64,
    max_items: Option<usize>,
    state: tauri::State<'_, CanvasState>,
) -> Result<Vec<CanvasAssignment>, String> {
    let auth = state.configured()?;
    let mut items = assignments(&state, &auth, course_id).await?;
    cap(&mut items, max_items);
    Ok(items)
}

#[tauri::command]
pub async fn canvas_list_due_dates(
    max_items: Option<usize>,
    state: tauri::State<'_, CanvasState>,
) -> Result<Vec<CanvasDueDate>, String> {
    let auth = state.configured()?;
    let courses = active_courses(&state, &auth).await?;
    if courses.len() > MAX_DUE_COURSES {
        return Err("Too many active courses for due-date aggregation".into());
    }
    let mut due = Vec::new();
    for course in courses {
        for item in assignments(&state, &auth, course.id).await? {
            if submission_complete(&item) {
                continue;
            }
            if let Some(due_at) = item.due_at.filter(|d| !d.is_empty()) {
                due.push(CanvasDueDate {
                    assignment_id: item.id,
                    course_id: course.id,
                    course_name: course.name.clone(),
                    name: item.name,
                    due_at,
                    html_url: item.html_url,
                });
            }
        }
    }
    due.sort_by(|a, b| {
        a.due_at
            .cmp(&b.due_at)
            .then(
                a.course_name
                    .to_lowercase()
                    .cmp(&b.course_name.to_lowercase()),
            )
            .then(a.assignment_id.cmp(&b.assignment_id))
    });
    due.dedup_by_key(|d| (d.course_id, d.assignment_id));
    if due.len() > MAX_ITEMS {
        return Err("Canvas due dates exceeded the safety limit".into());
    }
    cap(&mut due, max_items);
    Ok(due)
}

#[tauri::command]
pub async fn canvas_list_modules(
    course_id: u64,
    max_items: Option<usize>,
    state: tauri::State<'_, CanvasState>,
) -> Result<Vec<CanvasModule>, String> {
    let auth = state.configured()?;
    let mut url = endpoint(
        &auth.base_url,
        &format!("api/v1/courses/{course_id}/modules"),
    )?;
    url.query_pairs_mut()
        .append_pair("per_page", "100")
        .append_pair("include[]", "items")
        .append_pair("include[]", "content_details");
    let mut items = get_pages::<CanvasModule>(&state, &auth, url).await?;
    items.retain(|m| !m.name.trim().is_empty());
    items.sort_by_key(|m| (m.position, m.id));
    items.dedup_by_key(|m| m.id);
    cap(&mut items, max_items);
    Ok(items)
}

#[tauri::command]
pub async fn canvas_list_module_items(
    course_id: u64,
    module_id: u64,
    max_items: Option<usize>,
    state: tauri::State<'_, CanvasState>,
) -> Result<Vec<CanvasModuleItem>, String> {
    let auth = state.configured()?;
    let mut url = endpoint(
        &auth.base_url,
        &format!("api/v1/courses/{course_id}/modules/{module_id}/items"),
    )?;
    url.query_pairs_mut()
        .append_pair("per_page", "100")
        .append_pair("include[]", "content_details");
    let mut items = get_pages::<CanvasModuleItem>(&state, &auth, url).await?;
    items.retain(|m| !m.title.trim().is_empty());
    items.sort_by_key(|m| (m.position, m.id));
    items.dedup_by_key(|m| m.id);
    cap(&mut items, max_items);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_validation_is_strict() {
        assert!(validate_root("https://canvas.example.edu").is_ok());
        for bad in [
            "http://canvas.example.edu",
            "https://u:p@canvas.example.edu",
            "https://canvas.example.edu/path",
            "https://canvas.example.edu?q=x",
            "https://canvas.example.edu/#x",
        ] {
            assert!(validate_root(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn tokens_are_nonempty_and_single_piece() {
        assert_eq!(validate_token(" secret ").unwrap(), "secret");
        assert!(validate_token("  ").is_err());
        assert!(validate_token("secret value").is_err());
    }

    #[test]
    fn pagination_is_opaque_but_same_origin() {
        let root = Url::parse("https://canvas.example.edu/").unwrap();
        let current = Url::parse("https://canvas.example.edu/api/v1/courses?page=1").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::LINK, "<https://canvas.example.edu/custom/cursor?opaque=a%2Cb>; rel=\"next\", <x>; rel=\"last\"".parse().unwrap());
        assert_eq!(
            next_link(&headers, &current, &root)
                .unwrap()
                .unwrap()
                .path(),
            "/custom/cursor"
        );
        headers.insert(
            reqwest::header::LINK,
            "<https://evil.example/api>; rel=\"next\"".parse().unwrap(),
        );
        assert!(next_link(&headers, &current, &root).is_err());
    }

    #[test]
    fn result_caps_are_bounded() {
        assert_eq!(result_limit(None), 50);
        assert_eq!(result_limit(Some(0)), 1);
        assert_eq!(result_limit(Some(75)), 75);
        assert_eq!(result_limit(Some(10_000)), 100);
        let mut values: Vec<_> = (0..200).collect();
        cap(&mut values, Some(3));
        assert_eq!(values, vec![0, 1, 2]);
    }

    #[test]
    fn completed_and_excused_submissions_are_not_due() {
        let assignment =
            |workflow_state: &str, submitted_at: Option<&str>, excused| CanvasAssignment {
                id: 1,
                course_id: 2,
                name: "Work".into(),
                due_at: Some("2026-09-08T10:00:00Z".into()),
                html_url: None,
                points_possible: None,
                submission: Some(CanvasSubmission {
                    workflow_state: Some(workflow_state.into()),
                    submitted_at: submitted_at.map(str::to_owned),
                    excused: Some(excused),
                }),
                all_dates: Vec::new(),
            };
        assert!(submission_complete(&assignment("submitted", None, false)));
        assert!(submission_complete(&assignment(
            "unsubmitted",
            Some("2026-09-01"),
            false
        )));
        assert!(submission_complete(&assignment("unsubmitted", None, true)));
        assert!(!submission_complete(&assignment(
            "unsubmitted",
            None,
            false
        )));
    }

    #[test]
    fn assignment_nullables_deserialize() {
        let json = r#"[{"id":1,"course_id":2,"name":"Work","submission":{"workflow_state":null,"submitted_at":null,"excused":null},"all_dates":[{"id":null,"base":null,"title":null,"due_at":null}]}]"#;
        let assignments: Vec<CanvasAssignment> = serde_json::from_str(json).unwrap();
        assert_eq!(assignments.len(), 1);
        assert!(!submission_complete(&assignments[0]));
    }

    #[test]
    fn serialized_public_values_do_not_contain_token() {
        let status = CanvasStatus {
            connected: true,
            base_url: Some("https://canvas.example.edu".into()),
        };
        assert!(!serde_json::to_string(&status).unwrap().contains("token"));
    }
}
