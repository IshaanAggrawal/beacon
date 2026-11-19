// beacon demo client. No dependencies: SVG, fetch and EventSource.

const CELL = 24;
const COL = CELL * 5;
const ROW = CELL * 2;
const TOP = CELL * 3;
const LEFT = CELL * 2;
const SVGNS = "http://www.w3.org/2000/svg";
const SYSTEMS = [
  ["require_vouch", "Require a vouch"],
  ["denounce_only", "Denounce only"],
  ["peers_lists", "Adopt peers' denounce lists"],
  ["beacon", "beacon"],
];
const TIER_LABEL = { trusted: "Trusted", clean_newcomer: "Clean newcomer", quarantined: "Quarantined" };
const TRUTH_LABEL = {
  regular: "an established, honest contributor",
  newcomer: "an honest newcomer",
  campaign_spam: "part of a coordinated spam campaign",
  repeat_offender: "a repeat offender sending low-effort PRs",
};

const $ = (id) => document.getElementById(id);
const pct = (x) => (x > 0 && x < 0.005 ? "<1%" : `${Math.round(x * 100)}%`);

const state = {
  seed: 1,
  hostile: false,
  sybil: false,
  speed: 80,
  root: "ghostty",
  info: null,
  source: null,
  events: [],
  next: 0,
  playing: false,
  layout: null,
  graph: null,
  selected: null,
  verdict: null,
  lastGraphAt: 0,
  graphTimer: null,
  baseline: new Map(),
};

function query(extra = {}) {
  const p = new URLSearchParams({ seed: state.seed, hostile: state.hostile, sybil: state.sybil, ...extra });
  return p.toString();
}

async function getJSON(path, extra) {
  const resp = await fetch(`${path}?${query(extra)}`);
  if (!resp.ok) throw new Error(await resp.text());
  return resp.json();
}

// --- Scenario ----------------------------------------------------------------

async function loadScenario() {
  stop();
  state.events = [];
  state.next = 0;
  state.selected = null;
  state.verdict = null;
  state.layout = null;
  state.baseline.clear();
  $("stream").replaceChildren();
  $("stream-empty").hidden = false;
  $("card").hidden = true;
  $("play").textContent = "Replay 14 days";
  setClock("Loading the simulated federation…");

  state.info = await getJSON("/api/scenario");
  const root = $("root");
  const honest = state.info.projects.filter((p) => p.kind === "honest").map((p) => p.id);
  root.replaceChildren(...honest.map((id) => new Option(id, id, false, id === state.root)));
  if (!honest.includes(state.root)) state.root = "ghostty";
  root.value = state.root;

  renderScores();
  await refreshGraph(0, true);
  const attacks = [state.hostile && "a hostile project", state.sybil && "a 40-project Sybil ring"].filter(Boolean);
  setClock(
    `${state.info.prs} pull requests over ${state.info.days} days are ready.` +
      (attacks.length ? ` This run includes ${attacks.join(" and ")}.` : " Turn on an attack to see how the network copes."),
  );
}

function setClock(text) {
  $("clock").textContent = text;
}

// --- Replay ------------------------------------------------------------------

function play() {
  if (state.playing) return stop(true);
  if (state.next >= state.info.prs) {
    // Start over.
    state.events = [];
    state.next = 0;
    $("stream").replaceChildren();
    renderScores();
  }
  state.playing = true;
  $("play").textContent = "Pause";
  const src = new EventSource(`/api/stream?${query({ speed: state.speed, from: state.next })}`);
  state.source = src;
  src.addEventListener("pr", (msg) => onPr(JSON.parse(msg.data)));
  src.addEventListener("done", () => {
    stop();
    $("play").textContent = "Replay again";
    refreshGraph(state.info.prs - 1);
    setClock(clockText(state.events.at(-1), true));
  });
  src.onerror = () => {
    if (!state.playing) return;
    stop();
    setClock("Lost the connection to the demo server. Check that `beacon serve` is still running, then press replay.");
  };
}

function stop(paused = false) {
  if (state.source) state.source.close();
  state.source = null;
  state.playing = false;
  if (paused) $("play").textContent = "Resume";
}

function onPr(ev) {
  state.events.push(ev);
  state.next = ev.i + 1;
  $("stream-empty").hidden = true;
  if (matchesFilter(ev)) prependRow(ev);
  renderScores();
  setClock(clockText(ev, false));
  scheduleGraph(ev);
}

function dayOf(at) {
  const ms = new Date(at) - new Date(state.info.start);
  return Math.floor(ms / 86_400_000) + 1;
}

function timeOf(at) {
  return new Date(at).toISOString().slice(11, 16);
}

function clockText(ev, done) {
  if (!ev) return "";
  const n = state.events.length;
  const q = state.events.filter((e) => e.tier === "quarantined").length;
  const head = done ? `All ${state.info.days} days replayed.` : `Day ${dayOf(ev.at)} of ${state.info.days}, ${timeOf(ev.at)} UTC.`;
  return `${head} ${n} pull requests judged, ${q} quarantined by beacon.`;
}

// --- Stream ------------------------------------------------------------------

function matchesFilter(ev) {
  switch ($("filter").value) {
    case "quarantined": return ev.tier === "quarantined";
    case "review": return ev.review;
    case "newcomer": return ev.truth === "newcomer";
    default: return true;
  }
}

function rowFor(ev) {
  const li = document.createElement("li");
  const b = document.createElement("button");
  b.type = "button";
  b.dataset.i = ev.i;
  b.setAttribute("aria-pressed", String(state.selected === ev.i));
  const when = document.createElement("span");
  when.className = "when";
  when.textContent = `D${dayOf(ev.at)} ${timeOf(ev.at)}`;
  const who = document.createElement("span");
  who.className = "who";
  who.textContent = `${ev.author.replace("github:", "")} → ${ev.target}`;
  const tier = document.createElement("span");
  tier.className = `tier ${ev.tier}${ev.review || ev.single_source ? " warn" : ""}`;
  tier.textContent = TIER_LABEL[ev.tier];
  if (ev.review) tier.title = "Let through, but someone you trust denounced them";
  if (ev.single_source) tier.title = "This verdict rests on a single source";
  b.append(when, who, tier);
  b.addEventListener("click", () => select(ev.i));
  li.append(b);
  return li;
}

function prependRow(ev) {
  const list = $("stream");
  list.prepend(rowFor(ev));
  while (list.children.length > 300) list.lastElementChild.remove();
}

function rerenderStream() {
  const rows = state.events.filter(matchesFilter).slice(-300).reverse().map(rowFor);
  $("stream").replaceChildren(...rows);
  $("stream-empty").hidden = rows.length > 0 || state.events.length > 0;
}

// --- Scoreboard --------------------------------------------------------------

function renderScores() {
  const body = $("scores");
  const rows = SYSTEMS.map(([id, name]) => {
    const c = { spam: [0, 0], off: [0, 0], nc: [0, 0], honest: new Set(), smear: new Set() };
    for (const e of state.events) {
      const blocked = e.blocked[id];
      const bump = (k) => { c[k][1]++; if (blocked) c[k][0]++; };
      if (e.truth === "campaign_spam") bump("spam");
      if (e.truth === "repeat_offender") bump("off");
      if (e.truth === "newcomer") bump("nc");
      if (blocked && (e.truth === "regular" || e.truth === "newcomer")) {
        c.honest.add(e.author);
        if (e.smeared) c.smear.add(e.author);
      }
    }
    const share = ([hit, of]) => (of ? `${Math.round((hit / of) * 100)}%<span class="of">${hit} of ${of}</span>` : "—");
    const tr = document.createElement("tr");
    if (id === "beacon") tr.className = "beacon";
    tr.innerHTML = `
      <td>${name}</td>
      <td>${share(c.spam)}</td>
      <td>${share(c.off)}</td>
      <td class="${c.nc[0] ? "bad" : ""}">${share(c.nc)}</td>
      <td class="${c.honest.size ? "bad" : ""}">${c.honest.size}</td>
      <td class="${c.smear.size ? "bad" : ""}">${c.smear.size}</td>`;
    return tr;
  });
  body.replaceChildren(...rows);
}

// --- Graph -------------------------------------------------------------------

function scheduleGraph(ev) {
  // Refresh trust at most every 700 ms, sooner when a campaign is caught.
  const now = performance.now();
  const urgent = ev.campaign && ev.tier === "quarantined";
  if (urgent || now - state.lastGraphAt > 700) {
    clearTimeout(state.graphTimer);
    state.lastGraphAt = now;
    refreshGraph(ev.i);
  }
}

async function refreshGraph(upto, relayout = false) {
  const g = await getJSON("/api/graph", { root: state.root, upto });
  state.graph = g;
  if (relayout || !state.layout || state.layout.root !== g.root) state.layout = layout(g);
  drawGraph();
  updateReadout();
}

function layout(g) {
  const pos = new Map();
  const cols = new Map();
  const honest = g.nodes.filter((n) => n.kind !== "sybil");
  for (const n of honest) {
    const c = n.hops ?? 5;
    if (!cols.has(c)) cols.set(c, []);
    cols.get(c).push(n);
  }
  let height = TOP;
  for (const [c, nodes] of cols) {
    nodes.sort((a, b) => b.trust - a.trust || a.id.localeCompare(b.id));
    const step = ROW;
    nodes.forEach((n, r) => pos.set(n.id, { x: LEFT + c * COL, y: TOP + r * step }));
    height = Math.max(height, TOP + nodes.length * step);
  }
  const sybils = g.nodes.filter((n) => n.kind === "sybil");
  let ring = null;
  if (sybils.length) {
    const cx = LEFT + 3 * COL + CELL * 3;
    const cy = height + CELL * 4;
    const r = CELL * 2.5;
    sybils.forEach((n, k) => {
      const a = (k / sybils.length) * Math.PI * 2 - Math.PI / 2;
      pos.set(n.id, { x: Math.round(cx + r * Math.cos(a)), y: Math.round(cy + r * Math.sin(a)), small: true });
    });
    ring = { cx, cy, r, entrance: sybils[0].id };
    height = cy + r + CELL * 3;
  }
  return { root: g.root, pos, cols: [...cols.keys()], ring, width: LEFT + 5 * COL + CELL * 5, height: height + CELL };
}

function el(tag, attrs = {}, text) {
  const e = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  if (text != null) e.textContent = text;
  return e;
}

// Traces run along node centre lines and turn just left of the target
// column, where no label sits (labels are above their node).
function elbow(a, b) {
  const x1 = a.x + 8, x2 = b.x - 8;
  if (x2 > x1 + CELL) {
    return `M${x1} ${a.y}H${b.x - CELL}V${b.y}H${x2}`;
  }
  // Same column or backwards: loop out to the left of both.
  const out = Math.min(a.x, b.x) - CELL / 2 - 4;
  return `M${a.x - 8} ${a.y}H${out}V${b.y}H${b.x - 8}`;
}

function drawGraph() {
  const { layout: L, graph: g } = state;
  const svg = $("graph");
  svg.setAttribute("width", L.width);
  svg.setAttribute("height", L.height);
  svg.setAttribute("viewBox", `0 0 ${L.width} ${L.height}`);
  const byId = new Map(g.nodes.map((n) => [n.id, n]));
  const lit = litPaths();

  const labels = el("g");
  const names = { 0: "you", 1: "1 hop", 2: "2 hops", 3: "3 hops", 4: "4 hops", 5: "out of reach" };
  for (const c of L.cols) labels.append(el("text", { class: "col-label", x: LEFT + c * COL - 8, y: TOP - CELL * 1.75 }, names[c]));

  const edges = el("g");
  for (const e of g.edges) {
    const a = L.pos.get(e.from), b = L.pos.get(e.to);
    if (!a || !b) continue;
    const from = byId.get(e.from), to = byId.get(e.to);
    if (from.kind === "sybil" && to.kind === "sybil") continue; // drawn as the ring
    if (from.trust <= 0 || e.to === g.root) continue;
    const key = `${e.from}>${e.to}`;
    // Links pointing back toward the root carry no trust from it; only
    // draw them when they're part of a highlighted path.
    const backward = to.kind !== "sybil" && (to.hops ?? 9) <= (from.hops ?? 9);
    if (backward && !lit.has(key)) continue;
    const flow = from.trust * e.weight * (to.blame ? to.blame.multiplier : 1);
    const cls = ["edge"];
    cls.push(to.kind === "honest" ? "live" : "threat");
    if (lit.has(key)) cls.push("lit", lit.get(key));
    const path = to.kind === "sybil" && L.ring
      ? `M${a.x + 8} ${a.y}H${L.ring.cx}V${L.ring.cy - L.ring.r - 8}`
      : elbow(a, b);
    edges.append(el("path", {
      class: cls.join(" "),
      d: path,
      "stroke-width": lit.has(key) ? 3 : (0.75 + 3 * flow).toFixed(2),
      "stroke-opacity": lit.has(key) ? 1 : (0.15 + 0.6 * Math.min(1, flow)).toFixed(2),
    }));
  }

  const nodes = el("g");
  const hangs = new Set((state.verdict?.evidence.counterfactual.flips ?? []).map((f) => f.without));
  const litNodes = litNodeSet();
  for (const n of g.nodes) {
    const p = L.pos.get(n.id);
    if (!p) continue;
    const cls = ["node", n.kind];
    if (n.id === g.root) cls.push("root");
    if (n.trust <= 0 && n.id !== g.root) cls.push("cold");
    if (hangs.has(n.id)) cls.push("hangs");
    if (litNodes.has(n.id)) cls.push("lit", litNodes.get(n.id));
    const grp = el("g", { class: cls.join(" ") });
    const size = p.small ? 8 : 16;
    const h = n.id === g.root ? size : Math.round(size * Math.min(1, n.trust));
    grp.append(el("rect", { class: "body", x: p.x - size / 2, y: p.y - size / 2, width: size, height: size }));
    if (!p.small && h > 0) {
      grp.append(el("rect", { class: "fill", x: p.x - size / 2, y: p.y + size / 2 - h, width: size, height: h }));
    }
    const title = el("title", {}, describeNode(n, g.root));
    grp.append(title);
    if (!p.small) {
      grp.append(el("text", { x: p.x - 8, y: p.y - 13 }, n.id));
      if (n.id !== g.root) grp.append(el("text", { class: "pct", x: p.x - 8 + n.id.length * 6.7 + 6, y: p.y - 13 }, pct(n.trust)));
    }
    nodes.append(grp);
  }

  const extra = el("g");
  if (L.ring) {
    const entrance = byId.get(L.ring.entrance);
    const best = Math.max(...g.nodes.filter((n) => n.kind === "sybil").map((n) => n.trust));
    const count = g.nodes.filter((n) => n.kind === "sybil").length;
    const worth = best > 0 ? `worth ${pct(best)} to ${g.root}, all together` : `worth nothing to ${g.root}`;
    const lx = L.ring.cx - L.ring.r - CELL;
    extra.append(el("text", { class: "ring-label", x: lx, y: L.ring.cy + L.ring.r + CELL }, `Sybil ring: ${count} projects behind one link,`));
    extra.append(el("text", { class: "ring-label", x: lx, y: L.ring.cy + L.ring.r + CELL + 16 }, worth));
    if (entrance) extra.append(el("text", { class: "ring-label", x: L.ring.cx + 8, y: L.ring.cy - L.ring.r - 12 }, entrance.id));
  }

  svg.replaceChildren(labels, edges, nodes, extra);
}

function describeNode(n, root) {
  if (n.id === root) return `${n.id}: this is the point of view shown`;
  let s = `${n.id}: ${root} trusts it ${pct(n.trust)}`;
  if (n.path.length > 2) s += `, through ${n.path.slice(1, -1).join(", ")}`;
  if (n.blame) s += `. Lowered to ${pct(n.blame.multiplier)} of normal for bad calls`;
  return s;
}

function updateReadout() {
  const g = state.graph;
  const box = $("readout");
  const hostile = g.nodes.find((n) => n.kind === "hostile");
  if (!hostile) {
    box.hidden = true;
    return;
  }
  const first = state.baseline.get(`${g.root}>${hostile.id}`) ?? hostile.trust;
  if (!state.baseline.has(`${g.root}>${hostile.id}`)) state.baseline.set(`${g.root}>${hostile.id}`, hostile.trust);
  box.hidden = false;
  $("readout-label").textContent = `How much ${g.root} trusts ${hostile.id}`;
  $("readout-value").textContent = pct(hostile.trust);
  $("readout-value").classList.toggle("falling", hostile.trust < first - 0.005);
  const b = hostile.blame;
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
  const calls = b
    ? [
        b.vouched_for_flagged && `vouched for ${plural(b.vouched_for_flagged, "account", "accounts")} caught in a campaign`,
        b.denounced_your_people && `denounced ${plural(b.denounced_your_people, "person", "people")} ${g.root} vouches for`,
      ].filter(Boolean)
    : [];
  $("readout-why").textContent = calls.length
    ? `Cut by backward blame: it ${calls.join(" and ")}.`
    : hostile.trust > 0 ? "No bad calls noticed yet." : `${g.root} doesn't reach it.`;
}

function litPaths() {
  const m = new Map();
  const v = state.verdict;
  if (!v || v.target !== state.graph.root) return m;
  const mark = (sources, kind) => {
    for (const s of sources) for (let k = 0; k + 1 < s.path.length; k++) m.set(`${s.path[k]}>${s.path[k + 1]}`, kind);
  };
  mark(v.evidence.vouches.sources, "for");
  mark(v.evidence.denouncements.sources, "against");
  return m;
}

function litNodeSet() {
  const m = new Map();
  const v = state.verdict;
  if (!v || v.target !== state.graph.root) return m;
  for (const s of v.evidence.vouches.sources) m.set(s.project, "for");
  for (const s of v.evidence.denouncements.sources) m.set(s.project, "against");
  return m;
}

// --- Evidence card -------------------------------------------------------------

async function select(i) {
  state.selected = i;
  for (const b of $("stream").querySelectorAll("button")) b.setAttribute("aria-pressed", String(Number(b.dataset.i) === i));
  const card = $("card");
  card.hidden = false;
  card.innerHTML = `<div class="card-body"><p class="hint">Gathering the evidence…</p></div>`;
  const v = await getJSON("/api/verdict", { i });
  if (state.selected !== i) return;
  state.verdict = v;
  renderCard(v);
  if (v.target !== state.root) {
    state.root = v.target;
    $("root").value = v.target;
    await refreshGraph(i, true);
  } else {
    drawGraph();
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function sourceList(sources, root, side) {
  if (!sources.length) return `<dd>Nobody ${root} trusts.</dd>`;
  const items = sources.map((s) => {
    const who = s.project === root ? "you" : s.project;
    const via = s.path.length > 2 ? `, through ${s.path.slice(1, -1).map(esc).join(" → ")}` : "";
    const when = s.age_days == null ? ", date not recorded" : s.age_days === 0 ? ", today" : `, ${s.age_days} days ago`;
    const faded = s.strength < 0.99 ? `, faded to ${pct(s.strength)}` : "";
    const flags = [s.provisional && "provisional", s.under_appeal && "under appeal"].filter(Boolean).join(", ");
    const reason = s.reason ? `<span class="reason">“${esc(s.reason)}”</span>` : "";
    return `<li><span class="weight">${pct(s.weight)}</span> ${esc(who)}${via}${when}${faded}${flags ? `, ${flags}` : ""}${reason}</li>`;
  });
  return `<dd><ul class="${side}">${items.join("")}</ul></dd>`;
}

function renderCard(v) {
  const e = v.evidence;
  const by = v.summary.author.by === "llm"
    ? `Written by ${esc(v.summary.author.model)} from the evidence below.`
    : v.summary.author.why ? `Built-in summary (${esc(v.summary.author.why)}).` : "Built-in summary.";
  const c = e.campaign
    ? `${e.campaign.prs} near-identical PRs from ${e.campaign.accounts} accounts across ${e.campaign.repos} repos within ${Math.max(1, e.campaign.hours)} hours (${e.campaign.text_links} matched on wording, ${e.campaign.layout_links} on structure).`
    : "No match.";
  const cf = e.counterfactual;
  const check = cf.checked === 0
    ? "Nothing to check: nobody you trust has said anything about them."
    : cf.flips.length
      ? `<span class="hangs-on">Depends on ${cf.flips.map((f) => esc(f.without)).join(", ")}. Without ${cf.flips.length > 1 ? "any one of them" : "it"}, the answer would be “${TIER_LABEL[cf.flips[0].becomes].toLowerCase()}”. Checked in ${cf.micros} µs.</span>`
      : `Removing any one of the ${cf.checked} projects involved doesn't change this. Checked in ${cf.micros} µs.`;

  $("card").innerHTML = `
    <div class="card-body">
      <div class="card-title">
        <h3>${esc(v.author)} → ${esc(v.target)}</h3>
        <span class="tier ${e.tier}">${TIER_LABEL[e.tier]}</span>
      </div>
      <p class="card-pr">“${esc(v.title)}”</p>
      <div>
        <p class="summary">${esc(v.summary.text)}</p>
        <p class="summary-by">${by}</p>
      </div>
      <dl class="facts">
        <div><dt>Vouched for by</dt>${sourceList(e.vouches.sources, e.root, "for")}</div>
        <div><dt>Denounced by</dt>${sourceList(e.denouncements.sources, e.root, "against")}</div>
        <div><dt>Campaign</dt><dd>${c}</dd></div>
        <div><dt>Single-source check</dt><dd>${check}</dd></div>
      </dl>
      <p class="truth">In the simulation, this account is ${TRUTH_LABEL[v.truth]}. beacon didn't know that.</p>
    </div>`;
}

// --- Wiring --------------------------------------------------------------------

$("play").addEventListener("click", play);
for (const b of document.querySelectorAll(".speed button")) {
  b.addEventListener("click", () => {
    state.speed = Number(b.dataset.speed);
    for (const o of document.querySelectorAll(".speed button")) o.setAttribute("aria-checked", String(o === b));
    if (state.playing) { stop(); play(); }
  });
}
for (const id of ["hostile", "sybil"]) {
  $(id).addEventListener("change", (e) => {
    state[id] = e.target.checked;
    loadScenario().catch(fail);
  });
}
$("seed").addEventListener("change", (e) => {
  const n = Math.max(1, Math.min(999, Number(e.target.value) || 1));
  e.target.value = n;
  state.seed = n;
  loadScenario().catch(fail);
});
$("root").addEventListener("change", (e) => {
  state.root = e.target.value;
  refreshGraph(Math.max(0, state.next - 1), true).catch(fail);
});
$("filter").addEventListener("change", rerenderStream);

function fail(err) {
  setClock(`Something went wrong talking to the demo server: ${err.message}`);
}

// Links can set up a run: ?seed=2&hostile=1&sybil=1&play=320 (play = PRs per second).
const params = new URLSearchParams(location.search);
state.seed = Math.max(1, Math.min(999, Number(params.get("seed")) || 1));
state.hostile = params.get("hostile") === "1";
state.sybil = params.get("sybil") === "1";
$("seed").value = state.seed;
$("hostile").checked = state.hostile;
$("sybil").checked = state.sybil;
loadScenario()
  .then(() => {
    const speed = Number(params.get("play"));
    if (speed > 0) {
      state.speed = Math.min(1000, speed);
      play();
    }
  })
  .catch(fail);
