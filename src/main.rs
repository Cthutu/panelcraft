mod git;
mod github;
mod store;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use store::{Command, Store};
const URL: &str = "http://127.0.0.1:8765";

#[derive(Clone)]
struct Mcp {
    store: Store,
}
#[tool_router]
impl Mcp {
    #[tool(
        description = "Discover all PanelCraft spaces, their IDs, revisions and panel counts. Read a space with panelcraft to discover panel specifications before creating duplicates."
    )]
    fn list_spaces(&self) -> CallToolResult {
        self.result(
            self.store
                .execute(serde_json::from_value(json!({"action":"list"})).unwrap()),
        )
    }
    #[tool(
        description = "Manage isolated dashboard spaces, panels and shared JSON data. Use read with space_id to discover existing panels and their specifications/HTML/data. Actions: create_space/update_space/delete_space; create_panel (name,specification,html), update_panel (panel_id and any changed fields), delete_panel; set_data (key,value), delete_data (key). All except list/create_space require space_id. Pass expected_revision from read for safe writes. Panel HTML runs sandboxed: await panelcraft.getData(), await panelcraft.setData(key,value), await panelcraft.deleteData(key), panelcraft.subscribe(callback). No cross-space access or external scripts/network. create_panel and update_panel accept optional update_script: async JavaScript function body receiving data (space JSON snapshot) and html (previous markup), returning replacement HTML as a string or {setData:{key:value}} to patch space data atomically. Runs on user Refresh in an isolated worker without network or MCP access. For authenticated GitHub PRs put // @source github-authored-prs LOGIN on the first line and read source.pullRequests (GitHub GraphQL PullRequest nodes) and source.account. For local git branches put // @source local-git-branches [LOGIN] on the first line and read source.repos: [{repo, slug, default, branches:[{name, parent, current, worktree, ahead_of_parent, merged_into_default, unique_commits (not in default, cherry-pick aware), uncovered_commits (in neither default nor any open PR head), covered_by:[{number, head}] (open PRs whose head contains this branch), remote:{state:match|ahead|behind|diverged|none, ahead, behind}, pr:{number,state,isDraft,url,title,baseRefName}|null}]}] for the repositories configured on the server; LOGIN is the GitHub account whose PRs are matched. Attach only when the available inputs support a useful refresh; empty string removes the button. Save source and refresh instructions in specification; fetch external information using your own tools."
    )]
    fn panelcraft(&self, Parameters(command): Parameters<Command>) -> CallToolResult {
        self.result(self.store.execute(command))
    }
}
impl Mcp {
    fn result(&self, result: Result<Value, String>) -> CallToolResult {
        match result {
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
    }
}
#[tool_handler]
impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("PanelCraft", env!("CARGO_PKG_VERSION")))
    }
}
async fn command(State(store): State<Store>, Json(c): Json<Command>) -> Response {
    match tokio::task::spawn_blocking(move || store.execute(c)).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (
            if error.starts_with("Conflict:") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            },
            Json(json!({"error":error})),
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Storage task failed").into_response(),
    }
}
async fn local_only(req: Request, next: Next) -> Response {
    let host = req.headers().get("host").and_then(|v| v.to_str().ok());
    let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
    if !matches!(host, Some("127.0.0.1:8765" | "localhost:8765"))
        || origin.is_some_and(|o| o != URL && o != "http://localhost:8765")
    {
        return (
            StatusCode::FORBIDDEN,
            "Only local, same-origin access is allowed",
        )
            .into_response();
    }
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn panelcraft_is_running() -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let probe = async {
        let mut socket = tokio::net::TcpStream::connect("127.0.0.1:8765").await?;
        socket
            .write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1:8765\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = String::new();
        socket.take(4096).read_to_string(&mut response).await?;
        let healthy = response
            .split_once("\r\n\r\n")
            .is_some_and(|(headers, body)| {
                headers.split_whitespace().nth(1) == Some("200")
                    && serde_json::from_str::<Value>(body).ok()
                        == Some(json!({"application":"panelcraft"}))
            });
        Ok::<_, std::io::Error>(healthy)
    };
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), probe).await,
        Ok(Ok(true))
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg == "--open" {
        webbrowser::open(URL)?;
        return Ok(());
    }
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:8765").await {
        Ok(listener) => listener,
        Err(_) if arg == "--start" && panelcraft_is_running().await => {
            println!("PanelCraft is already running. Opening {URL}");
            webbrowser::open(URL)?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let path = std::env::var("PANELCRAFT_DB").unwrap_or_else(|_| "data/panelcraft.db".into());
    if let Some(parent) = std::path::Path::new(&path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let store = Store::open(&path)?;
    let mcp_store = store.clone();
    let mcp = StreamableHttpService::new(
        move || {
            Ok(Mcp {
                store: mcp_store.clone(),
            })
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../index.html")) }))
        .route(
            "/health",
            get(|| async { Json(json!({"application":"panelcraft"})) }),
        )
        .route("/api", post(command))
        .route("/sources/github-authored-prs", post(github::authored_prs))
        .route("/sources/local-git-branches", post(git::local_branches))
        .nest_service("/mcp", mcp)
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(middleware::from_fn(local_only))
        .with_state(store);
    println!(
        "\nPanelCraft is running\n\
         Dashboard: {URL}\n\
         Storage:   {path}\n\n\
         Connect your AI via MCP\n\
         In your MCP client's settings, add a server with:\n\
           Name:      PanelCraft\n\
           Transport: Streamable HTTP\n\
           URL:       {URL}/mcp\n\
           Auth:      None (local access only)\n\n\
         Connect, then refresh the client's tool list if needed.\n\
         Available tools: list_spaces, panelcraft\n\
         Try asking: \"List my PanelCraft spaces.\"\n\n\
         The MCP client must run on this machine; cloud clients cannot\n\
         reach this loopback address. Keep this server running while connected.\n\
         Press Ctrl+C to stop.\n"
    );
    if arg == "--start" {
        if let Err(e) = webbrowser::open(URL) {
            eprintln!("Could not open browser: {e}");
        }
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
