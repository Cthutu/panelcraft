# Getting started with PanelCraft

Extract the ZIP into a writable folder and open a terminal in that folder.
The executable contains the dashboard and its server. You can run it directly
without installing Rust or `just`.

On Windows, run:

```powershell
.\panelcraft.exe --start
```

On macOS or Linux, run:

```sh
chmod +x ./panelcraft
./panelcraft --start
```

This opens the dashboard at <http://127.0.0.1:8765>. Omit `--start` to serve
without opening a browser. Keep the server terminal open while using PanelCraft.
Press Ctrl+C there to stop it.

Create a space, then add a panel or choose **Try the league demo**.
Each panel has a name, a specification explaining its purpose, and HTML.
Panels in the same space can share data. The server serves the embedded
dashboard. The included `index.html` is a copy of that interface for reference.

## Connect your AI through MCP

In an MCP client running on the same machine, add a server with these settings.

| Setting | Value |
| --- | --- |
| Name | PanelCraft |
| Transport | Streamable HTTP |
| URL | `http://127.0.0.1:8765/mcp` |
| Authentication | None |

Connect or refresh the client's tools. Ask **“List my PanelCraft spaces.”**
The client should call `list_spaces`. An empty list is expected on a fresh
installation. To try a write, ask **“Create a Notes space with a panel for my
current tasks.”** The client uses the `panelcraft` tool and the new space
appears in the dashboard.

If the client cannot connect, open <http://127.0.0.1:8765/health> on the
server machine. A running server returns `{"application":"panelcraft"}`.
The MCP endpoint is local to that machine. A hosted client needs a local
connection mechanism to reach it.

## Refresh a GitHub panel

For a GitHub PR refresh panel, install GitHub CLI on the machine running
PanelCraft and sign in with `gh auth login`. The script's first line declares
`// @source github-authored-prs YOUR-LOGIN`. Press **Refresh** to fetch the
signed-in account's open PRs and reviewer details.

## Keep your data

For the local branch source, put one repository directory per line in
`git-repos.txt` beside the database, or set `PANELCRAFT_GIT_REPOS` using your
platform's PATH separator. Use `// @source local-git-branches` as the refresh
script's first line. Append a GitHub login to include PR metadata from `gh`.
Remote comparisons use the last fetched refs. Refresh does not fetch or prune.

PanelCraft saves spaces, panels and shared data in `data/panelcraft.db`
relative to the directory where you start the server. Use the same working
directory on subsequent runs, or set `PANELCRAFT_DB` to an absolute database
path. Stop the server before backing up the data folder or replacing the
executable during an upgrade.
