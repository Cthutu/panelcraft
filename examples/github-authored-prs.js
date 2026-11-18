// @source github-authored-prs YOUR-LOGIN
// PanelCraft refresh function body. Replace YOUR-LOGIN with your gh login.
const escape = value => String(value).replace(/[&<>"']/g, c =>
  ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
const rows = source.pullRequests.map(pr => {
  const repo = pr.repository.nameWithOwner;
  const requests = pr.reviewRequests.nodes.map(({requestedReviewer:r}) =>
    r?.login || (r?.slug ? r.organization.login + "/" + r.slug : null)).filter(Boolean);
  const reviews = pr.latestReviews.nodes.map(r =>
    "@" + (r.author?.login || "unknown") + " (" + r.state.toLowerCase().replaceAll("_"," ") + ")");
  const review = {APPROVED:["Approved","approved"], CHANGES_REQUESTED:["Changes requested","changes"],
    REVIEW_REQUIRED:["Review required","pending"]}[pr.reviewDecision] ||
    (requests.length ? ["Review requested","pending"] : ["No decision","neutral"]);
  const people = [requests.length && "Requested: " + requests.map(x=>"@"+x).join(", "),
    reviews.length && "Reviewed: " + reviews.join(", ")].filter(Boolean).join(" · ");
  const badges = '<b class="open">Open</b>' + (pr.isDraft ? '<b class="neutral">Draft</b>' : "") +
    '<b class="' + review[1] + '">' + review[0] + '</b>';
  return '<article><div><small>' + escape(repo) + ' #' + pr.number + '</small><br><a href="' +
    escape(pr.url) + '" target="_blank" rel="noopener noreferrer">' + escape(pr.title) +
    '</a><br><small>' + escape(people || "No reviewers") + '</small></div><aside>' + badges + '</aside></article>';
});
return `<style>body{margin:12px;font:13px/1.4 system-ui}header,article{display:flex;align-items:center;justify-content:space-between;gap:8px}article{border-top:1px solid #ddd;padding:7px 0}aside{display:flex;flex-wrap:wrap;gap:4px;justify-content:flex-end}b{white-space:nowrap;padding:2px 5px;font-size:11px;border-radius:4px}.open{background:#dcecff}.approved{background:#d4edda}.changes{background:#ffddd4}.pending{background:#fff0c0}.neutral{background:#eee}a{color:#205d82}</style>
<header><strong>${rows.length} open</strong><small>Checked ${new Date().toISOString().slice(0,16).replace("T"," ")} UTC</small></header>${rows.join("")}`;
