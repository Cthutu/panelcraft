# PanelCraft

A local dashboard maintained by you and your LLM. The entire interface is `index.html`, with embedded CSS and JavaScript. A Rust/Axum companion server serves that file, stores state in SQLite, and exposes MCP using the official Rust SDK (`rmcp`).

## Run

Install Rust and `just`, then:

```sh
just start
```

The first run compiles the server and opens http://127.0.0.1:8765 once it is listening. Both `just start` and `just serve` print MCP connection instructions, including the transport, URL, authentication setting, available tools, and a sample prompt. Keep the terminal running; Ctrl+C stops the server.

- `just serve`: run the server without opening a browser.
- `just open`: open the UI of an already running server.
- `just test`: run Rust storage tests.

Create a space or select **Try the league demo**. The demo contains sample fixtures and a league table; saving a score updates both through shared data. No actual GitHub or game data is fetched automatically.

## Connect an LLM

Configure an MCP client that supports local Streamable HTTP with URL:

```text
http://127.0.0.1:8765/mcp
```

The server must run on the machine the MCP client can reach; a cloud-only client cannot reach your laptop's loopback address. This prototype is local-only and does not configure the client's MCP settings automatically.

Tools:

- `list_spaces`: discover names, IDs, revisions and panel counts.
- `panelcraft`: typed actions below. `read` returns panels including specifications and HTML, along with the space's shared data.

| Action | Required fields |
| --- | --- |
| `list` | None |
| `create_space` | `name` |
| `read`, `delete_space` | `space_id` |
| `update_space` | `space_id`, `name` |
| `create_panel` | `space_id`, `name`, `specification`, `html` |
| `update_panel` | `space_id`, `panel_id`, fields to change: `name`, `specification`, `html` |
| `delete_panel` | `space_id`, `panel_id` |
| `set_data` | `space_id`, `key`, `value` (JSON, including null) |
| `delete_data` | `space_id`, `key` |

Pass `expected_revision` on writes after reading the space to detect concurrent changes. A conflict makes no changes; read again and reconcile. Name and specification are nonempty. Names need not be unique: find existing items by purpose, then use their stable IDs. An update preserves unspecified fields.

Example instruction: “Find my PRs space. Read its panels and specifications. Fetch my authored PRs and review requests using GitHub, then update the existing panels with their latest statuses.” Include the data source and refresh procedure in each panel's specification so another session can resume the work.

## Panel API

Panel HTML can include inline styles and scripts. The host injects this API before panel code executes:

```html
<div id="score"></div>
<script>
  function render(data) {
    document.getElementById('score').textContent = data.score ?? 0;
  }
  panelcraft.getData().then(render);
  const unsubscribe = panelcraft.subscribe(render);
  // In an interaction handler:
  // await panelcraft.setData('score', 3);
  // await panelcraft.deleteData('score');
</script>
```

Shared data is a JSON object; each key is replaced atomically. `setData` and `deleteData` resolve with the updated data. Handle rejected promises to show conflicts/errors. `subscribe` returns an unsubscribe function. The page polls the server every 1.5 seconds; data changes notify panels without reloading them. Changing a panel's HTML reloads that frame.

Each panel belongs to exactly one space. The host identifies frames by their window, never by a space ID supplied by panel code. Sandboxed frames cannot access the parent DOM or other spaces; a restrictive CSP blocks fetch, external scripts, styles, and forms. Inline code/styles and data-URL images are supported. Do not treat this as isolation from resource exhaustion by deliberately hostile scripts.

## Storage and development

State lives in `data/panelcraft.db` (SQLite, WAL mode). Stop the server before copying the database as a backup. Set `PANELCRAFT_DB` to choose another path. Deleting a space deletes its panels and data. There is no undo in this first version.

HTML is embedded at Rust compile time; restart with `just serve` after editing it. The HTML needs the companion API and should be opened through the server, not `file://`. It has no CDN dependencies or frontend build step.

The server binds only to 127.0.0.1 and rejects foreign Host/Origin headers. It trusts local non-browser clients; authentication and remote hosting are outside this initial implementation.

Optional browser integration tests (Node.js required): `npm ci`, `npx playwright install chromium`, then `npm test`. Tests launch their own server with a temporary database on port 8765, so stop any running instance first. They cover UI editing, demo score propagation, isolation, persistence, and MCP CRUD. Screenshots are written to `test-results/`.
