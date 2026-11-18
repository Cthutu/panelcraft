import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { request } from "node:http";

const base = "http://127.0.0.1:8765";
try {
  await fetch(base + "/health");
  throw Error("Stop the existing server before running tests.");
} catch (e) {
  if (!e.cause) throw e;
}
const temp = await mkdtemp(join(tmpdir(), "panelcraft-test-"));
const fixtureRepo = join(temp, "local-repo");
let server, browser;
async function start() {
  server = spawn(
    resolve(
      "target/debug/panelcraft" + (process.platform === "win32" ? ".exe" : ""),
    ),
    [],
    {
      env: { ...process.env, PANELCRAFT_DB: join(temp, "test.db"), PANELCRAFT_GIT_REPOS:fixtureRepo },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let logs = "";
  server.stderr.on("data", (d) => (logs += d));
  for (let n = 0; n < 100; n++) {
    try {
      if ((await fetch(base + "/health")).ok) return;
    } catch {}
    if (server.exitCode !== null) throw Error(logs);
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error("Server startup timeout: " + logs);
}
async function stop() {
  if (server && server.exitCode === null) {
    const done = once(server, "exit");
    server.kill();
    await done;
  }
}
async function api(action, args = {}) {
  const r = await fetch(base + "/api", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ action, ...args }),
  });
  const v = await r.json();
  if (!r.ok) throw Error(v.error);
  return v;
}
let rpcId = 0;
async function rpc(method, params) {
  const r = await fetch(base + "/mcp", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      "MCP-Protocol-Version": "2025-11-25",
    },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++rpcId, method, params }),
  });
  const text = await r.text();
  assert.equal(r.status, 200, text);
  const v = JSON.parse(text);
  assert.ok(!v.error, JSON.stringify(v));
  return v.result;
}
async function tool(args) {
  const r = await rpc("tools/call", { name: "panelcraft", arguments: args });
  assert.ok(!r.isError, JSON.stringify(r));
  return JSON.parse(r.content[0].text);
}
try {
  await mkdir(fixtureRepo);
  const git = (...args) => execFileSync("git", ["-C", fixtureRepo, ...args]);
  git("init", "-b", "main");
  git("-c","user.name=Tester","-c","user.email=test@example.org","-c","commit.gpgSign=false","commit","--allow-empty","-m","Initial");
  await start();
  const init = await rpc("initialize", {
    protocolVersion: "2025-11-25",
    capabilities: {},
    clientInfo: { name: "panelcraft-test", version: "1" },
  });
  assert.equal(init.serverInfo.name, "PanelCraft");
  const tools = await rpc("tools/list", {});
  assert.equal(tools.tools.length, 2);
  const schema = tools.tools.find(t => t.name === "panelcraft").inputSchema;
  assert.ok(Object.hasOwn(schema.properties,"update_script"));
  const space = await tool({ action: "create_space", name: "MCP test" });
  let state = await tool({
    action: "create_panel",
    space_id: space.id,
    name: "Test",
    specification: "Testing MCP persistence",
    html: "<h2>Original</h2>",
  });
  state = await tool({
    action: "update_panel",
    space_id: space.id,
    panel_id: state.panels[0].id,
    html: "<h2>Updated</h2>",
    expected_revision: state.revision,
  });
  assert.equal(state.panels[0].specification, "Testing MCP persistence");
  state = await tool({
    action: "set_data",
    space_id: space.id,
    key: "secret",
    value: "space A only",
  });
  await stop();
  await start();
  assert.equal(
    (await tool({ action: "read", space_id: space.id })).data.secret,
    "space A only",
  );
  await tool({
    action: "delete_panel",
    space_id: space.id,
    panel_id: state.panels[0].id,
  });
  await tool({ action: "delete_space", space_id: space.id });
  assert.equal((await api("list")).length, 0);
  for (const headers of [
    { Origin: "null" },
    { Origin: "https://example.org" },
  ]) {
    const r = await fetch(base + "/api", {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: '{"action":"list"}',
    });
    assert.equal(r.status, 403, JSON.stringify(headers));
  }
  const foreignHostStatus = await new Promise((resolve, reject) => {
    const req = request(
      base + "/health",
      { headers: { Host: "example.org" } },
      (r) => {
        r.resume();
        resolve(r.statusCode);
      },
    );
    req.on("error", reject);
    req.end();
  });
  assert.equal(foreignHostStatus, 403);
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 960 },
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(base);
  await page.getByText("Server connected", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Try the league demo" }).click();
  await page
    .getByRole("heading", { name: "Sunday League · Demo", exact: true })
    .waitFor();
  const standings = page.frameLocator('iframe[title="League table"]');
  const fixtures = page.frameLocator('iframe[title="Fixtures & results"]');
  await standings
    .getByRole("cell", { name: "Northside", exact: true })
    .waitFor();
  await fixtures.getByLabel("Park Athletic score").first().fill("3");
  await fixtures.getByLabel("Old Town score").first().fill("0");
  await fixtures
    .getByRole("button", { name: "Save", exact: true })
    .nth(1)
    .click();
  await standings
    .getByRole("row")
    .filter({ hasText: "Park Athletic" })
    .getByRole("cell", { name: "3", exact: true })
    .first()
    .waitFor();
  const league = (await api("list"))[0];
  assert.equal(
    (await api("read", { space_id: league.id })).data.matches[1].h,
    3,
  );
  await mkdir("test-results", { recursive: true });
  await page.screenshot({ path: "test-results/desktop.png", fullPage: true });
  // Panel HTML and specification edits through the actual editor.
  await page
    .getByRole("button", { name: "Edit ↗", exact: true })
    .first()
    .click();
  await page.locator("#edit-name").fill("Standings");
  await page.locator("#edit-spec").fill("A revised specification");
  await page.getByRole("button", { name: "Save changes" }).click();
  await page.getByRole("heading", { name: "Standings", exact: true }).waitFor();
  // Create another isolated space and test a panel's view of shared data.
  const other = await api("create_space", { name: "Private space" });
  await api("set_data", { space_id: other.id, key: "private", value: true });
  await api("create_panel", {
    space_id: other.id,
    name: "Isolation check",
    specification: "Isolation test",
    html: '<div id="value"></div><script>panelcraft.getData().then(d=>document.getElementById("value").textContent=JSON.stringify(d));</script>',
  });
  await page.getByRole("button", { name: /Private space/ }).click();
  const frame = page.frameLocator('iframe[title="Isolation check"]');
  await frame.getByText('{"private":true}', { exact: true }).waitFor();
  const content = page.frames().find((f) => f.parentFrame());
  assert.equal(
    await content.evaluate(() => {
      try {
        parent.document.body;
        return false;
      } catch {
        return true;
      }
    }),
    true,
  );
  // Stale panel write must fail, including when the host has not polled yet.
  await content.evaluate(() => panelcraft.getData());
  await api("set_data", { space_id: other.id, key: "private", value: "newer" });
  const stale = await content.evaluate(async () => {
    try {
      await panelcraft.setData("private", "stale");
      return false;
    } catch (e) {
      return e.message.includes("Conflict");
    }
  });
  assert.equal(stale, true);
  // Direct foreign-space API access from a panel must fail.
  assert.equal(
    await content.evaluate(async () => {
      try {
        await fetch("/api", { method: "POST", body: "{}" });
        return false;
      } catch {
        return true;
      }
    }),
    true,
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: "test-results/mobile.png", fullPage: true });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  );
  assert.deepEqual(errors, []);
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByRole("button", { name: "Dark mode" }).click();
  await page.reload();
  assert.equal(await page.locator("html").getAttribute("data-theme"), "dark");
  const manageSpace = page.getByRole("button", { name: "Manage space" });
  async function reviewDeletion() {
    await manageSpace.click();
    await page.locator("#editor").getByRole("button", { name: "Delete space" }).click();
    await page.locator("#delete-space-dialog[open]").waitFor();
  }
  await reviewDeletion();
  assert.match(await page.locator("#delete-space-title").innerText(), /Private space/);
  assert.deepEqual(await page.locator("#delete-space-panels li").allTextContents(), ["Isolation check"]);
  await page.locator("#delete-space-dialog").getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal((await api("read", { space_id: other.id })).panels.length, 1);
  await reviewDeletion();
  await api("create_panel", { space_id: other.id, name: "Added during confirmation", specification: "Test stale confirmation", html: "" });
  await page.locator("#confirm-delete-space").click();
  await page.locator("#delete-space-error").getByText(/Conflict/).waitFor();
  await page.locator("#delete-space-dialog").getByRole("button", { name: "Cancel", exact: true }).click();
  await reviewDeletion();
  assert.equal(await page.locator("#delete-space-panels li").count(), 2);
  await page.locator("#confirm-delete-space").click();
  await page.locator("#delete-space-dialog").waitFor({ state: "hidden" });
  assert.ok(!(await api("list")).some(space => space.id === other.id));
  const before = await api("read", {space_id:league.id});
  const [first, second] = before.panels;
  const cards = page.locator(".panel");
  await cards.first().locator(".grip").dragTo(cards.nth(1).locator("header"));
  await page.waitForFunction(name => document.querySelector(".panel h2")?.textContent === name, second.name);
  const after = await api("read", {space_id:league.id});
  assert.equal(after.panels[0].id, second.id);
  await page.locator("#mark-read").click();
  await api("update_panel", {space_id:league.id,panel_id:first.id,specification:"Changed after acknowledgement"});
  await cards.filter({has:page.getByRole("heading",{name:first.name,exact:true})}).locator(".updated:visible").waitFor();
  const changedCard = cards.filter({has:page.getByRole("heading",{name:first.name,exact:true})});
  const otherCard = cards.filter({has:page.getByRole("heading",{name:second.name,exact:true})});
  const tabBadge = page.locator("#spaces .active .updated");
  await api("update_panel", {space_id:league.id,panel_id:second.id,specification:"Second unread panel"});
  await otherCard.locator(".updated:visible").waitFor();
  await changedCard.getByRole("button",{name:"Mark this panel as seen"}).press("Enter");
  await changedCard.locator(".updated").waitFor({state:"hidden"});
  assert.ok(await tabBadge.isVisible());
  const inside = otherCard.frameLocator("iframe").locator("body");
  await inside.click({position:{x:3,y:3}});
  await tabBadge.waitFor({state:"hidden"});

  // A press begun before another update must not acknowledge the newer revision.
  await api("update_panel",{space_id:league.id,panel_id:first.id,specification:"Older revision"});
  await changedCard.locator(".updated:visible").waitFor();
  const heading = changedCard.locator("h2");
  await heading.hover();
  await page.mouse.down();
  await api("update_panel",{space_id:league.id,panel_id:first.id,specification:"Newer revision"});
  await page.waitForFunction(() => !!document.querySelector('h2[title="Newer revision"]'));
  await page.mouse.up();
  assert.ok(await changedCard.locator(".updated").isVisible());
  await heading.click();
  await tabBadge.waitFor({state:"hidden"});
  await api("update_space",{space_id:league.id,name:before.name});
  await tabBadge.waitFor();
  assert.equal(await cards.locator(".updated:visible").count(),0);
  await page.locator("#mark-read").click();
  const extra = await api("create_space", {name:"Reorder test"});
  const tabs = page.locator("#spaces button");
  await tabs.filter({hasText:extra.name}).dragTo(tabs.first());
  await page.waitForFunction(() => document.querySelector("#spaces button")?.textContent.includes("Reorder test"));
  await page.reload();
  await tabs.filter({hasText:before.name}).click();
  await page.getByRole("heading",{name:second.name,exact:true}).waitFor();
  assert.match(await tabs.first().textContent(), /Reorder test/);
  assert.equal(await cards.first().locator("h2").textContent(), second.name);
  assert.equal(await cards.locator(".updated:visible").count(),0);
  const refreshCard = cards.filter({has:page.getByRole("heading",{name:first.name,exact:true})});
  assert.equal(await refreshCard.getByRole("button",{name:"Refresh",exact:true}).count(),0);
  await api("update_panel",{space_id:league.id,panel_id:first.id,
    update_script:"await new Promise(r => setTimeout(r, 350)); return '<h3>Updated standings</h3>';"});
  const button = refreshCard.getByRole("button",{name:"Refresh",exact:true});
  await button.waitFor();
  await page.locator("#mark-read").click();
  await button.click();
  assert.ok(await refreshCard.getByRole("button",{name:"Refreshing\u2026"}).isDisabled());
  await refreshCard.frameLocator("iframe").getByText("Updated standings").waitFor();
  await refreshCard.locator(".updated:visible").waitFor();
  await page.locator("#spaces .active .updated").waitFor();
  await api("update_panel",{space_id:league.id,panel_id:first.id, update_script:"throw Error('Example failure')"});
  await page.waitForFunction(() => current.panels.some(p => p.update_script?.includes("Example failure")));
  await page.locator("#mark-read").click();
  await button.click();
  await page.getByText("Panel refresh failed: Example failure").waitFor();
  assert.equal(await refreshCard.locator(".updated:visible").count(),0);
  assert.ok(await refreshCard.frameLocator("iframe").getByText("Updated standings").isVisible());
  git("branch","new-dashboard-branch");
  const renderer = (await api("read",{space_id:league.id})).panels.find(p=>p.id===first.id).html;
  await api("update_panel",{space_id:league.id,panel_id:first.id,update_script:
    "// @source local-git-branches\nreturn {setData:{branches:source.repos,repoCount:source.repos.length}};"});
  await page.waitForFunction(() => current.panels.some(p=>p.update_script?.includes("repoCount")));
  await button.click();
  await page.waitForFunction(()=>current?.data.repoCount===1);
  const refreshed = await api("read",{space_id:league.id});
  assert.ok(refreshed.data.branches[0].branches.some(b=>b.name==="new-dashboard-branch"));
  assert.equal(refreshed.panels.find(p=>p.id===first.id).html,renderer);
  assert.ok(refreshed.data.matches);
  await page.locator("#mark-read").click();
  await button.click();
  await page.getByText("Already up to date",{exact:true}).waitFor();
  assert.equal((await api("read",{space_id:league.id})).revision,refreshed.revision);
  assert.equal(await cards.locator(".updated:visible").count(),0);

  await page.setViewportSize({width:1200,height:850});
  const rect = await cards.first().boundingBox();
  assert.ok(rect.height>600 && rect.width>=400);
  await page.setViewportSize({width:2400,height:1800});
  const large = await cards.first().boundingBox();
  assert.ok(large.width>1000 && large.height>1500);
  const addTile = name => api("create_panel",{space_id:league.id,name,specification:"Layout trial",html:name});
  await addTile("Third");
  await addTile("Fourth");
  await page.setViewportSize({width:3440,height:1277});
  await page.getByRole("heading",{name:"Fourth",exact:true}).waitFor();
  let boxes=await cards.evaluateAll(nodes=>nodes.map(n=>n.getBoundingClientRect().toJSON()));
  assert.equal(boxes[0].top,boxes[1].top);
  assert.ok(boxes[2].top>boxes[0].top && boxes[0].width>1600);
  assert.ok(1277-boxes[3].bottom<=18);
  await addTile("Wide table");
  const table=page.getByRole("heading",{name:"Wide table",exact:true});
  await table.waitFor();
  await page.locator("#edit-layout").click();
  await page.locator("#arrangement").selectOption("right");
  await page.locator("#feature").selectOption({label:"Wide table"});
  const divider = await page.locator("#divider").boundingBox();
  await page.mouse.move(divider.x+4,divider.y+20);
  await page.mouse.down();
  await page.mouse.move(divider.x+250,divider.y+20);
  await page.mouse.up();
  assert.ok(Number(await page.locator("#split").inputValue())<48);
  await page.locator("#layout-editor").getByRole("button",{name:"Save layout"}).click();
  await page.locator("#layout-editor").waitFor({state:"hidden"});
  await page.reload();
  await table.waitFor();
  boxes=await cards.evaluateAll(nodes=>nodes.map(n=>n.getBoundingClientRect().toJSON()));
  assert.ok(boxes[4].left>boxes[1].left && boxes[4].height>boxes[0].height*1.9);
  assert.ok(Math.abs(boxes[4].bottom-boxes[3].bottom)<2);

  await page.setViewportSize({width:360,height:650});
  const small = await cards.first().boundingBox();
  assert.ok(small.width<=360 && small.height>=240);
  console.log(
    "PASS: MCP CRUD, restart persistence, origin protection, UI edits, league updates, space isolation, stale-write protection, mobile layout.",
  );
} finally {
  await browser?.close();
  await stop();
  await rm(temp, { recursive: true, force: true });
}
