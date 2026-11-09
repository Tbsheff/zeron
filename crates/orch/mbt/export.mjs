import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRATCH = join(process.env.HOME, ".zeron", "scratch", "2026-11-08-19843a51", "tla");
const SPEC_DIR = process.env.ORCH_SPEC_DIR ?? join(SCRATCH, "v2");
const TLA_TS = process.env.ORCH_TLA_TS ?? join(SCRATCH, "node_modules", ".bin", "tla-ts");
const OUT = join(HERE, "fixtures");
const WORKERS = process.env.ORCH_TLA_WORKERS ?? "4";
const SPEC_FILES = ["RunBase.tla", "RunLifecycle.tla", "Delegation.tla"];
const DROP = new Set(["kidIn", "doomed", "replayable", "replayBad", "receiptBad", "qPend", "qAt", "qVis", "qWake", "qUsed", "answers"]);

const RL_BASE = { Threads: "{t1}", Root: "t1", NoThread: "NoThread", Mut: '"none"', FixLimitRelease: "TRUE" };
const DEL_BASE = {
  Root: "r", NoThread: "NoThread", MaxQueue: 2, Mut: '"none"', MaxGen: 3, Crash: "TRUE", LateAck: "TRUE",
  Questions: "FALSE", FixWakeSkipsFailHold: "TRUE", StopCheckAnc: "FALSE", PartialCrash: "FALSE",
};

const rl = (name, c) => ({ name, module: "RunLifecycle", spec: "RLSpec", invariants: ["RL_BoundsNotHit", "S1_AtMostOneActiveRun"], constants: { ...RL_BASE, ...c } });
const del = (name, c) => ({ name, module: "Delegation", spec: "DSpec", invariants: ["D_BoundsNotHit", "S1_AtMostOneActiveRun", "S3_ExactlyOnce", "S4_NoWakeIntoLiveRun", "S5_DepthFanout", "S6_AccessNarrow", "S7_AckFromLiveRun"], constants: { ...DEL_BASE, ...c } });

const RL_ZERO = { MaxQueue: 2, MaxPerson: 1, MaxServer: 0, MaxRetry: 0, MaxStop: 1, MaxUL: 0, MaxCrash: 0, MaxRuns: 4, MaxAtt: 5 };
const DEL_ZERO = { MaxRuns: 5, MaxDepth: 1, MaxFanout: 1, MaxPerson: 1, MaxFail: 0, MaxCrash: 0, MaxAbandon: 0, MaxStop: 0, MaxNarrow: 0, MaxInterrupt: 0, DupDelivery: "FALSE" };

export const CONFIGS = [
  rl("rl_steer", { ...RL_ZERO, MaxPerson: 2 }),
  rl("rl_retry", { ...RL_ZERO, MaxQueue: 1, MaxRetry: 1, MaxCrash: 1, MaxRuns: 3 }),
  rl("rl_usage_limit", { ...RL_ZERO, MaxQueue: 1, MaxServer: 1, MaxUL: 1, MaxAtt: 4 }),
  rl("rl_limit_hold", { ...RL_ZERO, MaxPerson: 2, MaxUL: 1, MaxAtt: 4 }),
  rl("rl_server", { ...RL_ZERO, MaxServer: 1, MaxStop: 0, MaxUL: 1, MaxAtt: 4 }),
  del("del_pair", { ...DEL_ZERO, Threads: "{r, a}", MaxRuns: 6, MaxFail: 1, MaxAbandon: 1, MaxStop: 1, MaxNarrow: 1, MaxInterrupt: 1 }),
  del("del_replay", { ...DEL_ZERO, Threads: "{r, a}", MaxRuns: 6, MaxCrash: 1, MaxStop: 1, DupDelivery: "TRUE" }),
  del("del_chain", { ...DEL_ZERO, Threads: "{r, a, b}", MaxRuns: 4, MaxDepth: 2, MaxNarrow: 1 }),
  del("del_fanout", { ...DEL_ZERO, Threads: "{r, a, b}", MaxFanout: 2 }),
];

function cfgText(job) {
  const consts = Object.entries(job.constants).map(([k, v]) => `  ${k} = ${v}`);
  return ["SPECIFICATION " + job.spec, "CONSTANTS", ...consts, "INVARIANTS", ...job.invariants.map((i) => "  " + i), ""].join("\n");
}

function runChecker(job, dir) {
  for (const f of SPEC_FILES) copyFileSync(join(SPEC_DIR, f), join(dir, f));
  writeFileSync(join(dir, "MBT.cfg"), cfgText(job));
  const args = ["-workers", WORKERS, "-dump", "dot,actionlabels", "graph.dot", "-config", "MBT.cfg", `${job.module}.tla`];
  const r = spawnSync(TLA_TS, args, { cwd: dir, encoding: "utf8", maxBuffer: 1 << 28 });
  const log = `${r.stdout ?? ""}${r.stderr ?? ""}`;
  if (!/No error has been found/.test(log)) throw new Error(`${job.name}: checker did not pass\n${log.slice(-2000)}`);
  return readFileSync(join(dir, "graph.dot"), "utf8");
}

function unescapeDot(s) {
  return s.replace(/\\(["\\n])/g, (_, c) => (c === "n" ? "\n" : c));
}

function labelOf(line) {
  const m = line.match(/\[label="((?:[^"\\]|\\.)*)"/);
  return m ? unescapeDot(m[1]) : null;
}

class Parser {
  constructor(text) {
    this.s = text;
    this.i = 0;
  }
  ws() {
    while (this.i < this.s.length && /\s/.test(this.s[this.i])) this.i++;
  }
  peek(tok) {
    this.ws();
    return this.s.startsWith(tok, this.i);
  }
  eat(tok) {
    if (!this.peek(tok)) throw new Error(`expected ${tok} at ${this.i}: ${this.s.slice(this.i, this.i + 40)}`);
    this.i += tok.length;
  }
  list(close, item) {
    const out = [];
    if (this.peek(close)) {
      this.eat(close);
      return out;
    }
    for (;;) {
      out.push(item());
      if (this.peek(close)) break;
      this.eat(",");
    }
    this.eat(close);
    return out;
  }
  value() {
    this.ws();
    const c = this.s[this.i];
    if (this.peek("<<")) return this.seq();
    if (c === "{") return this.set();
    if (c === "[") return this.record();
    if (c === "(") return this.func();
    if (c === '"') return this.str();
    return this.atom();
  }
  seq() {
    this.eat("<<");
    return this.list(">>", () => this.value());
  }
  set() {
    this.eat("{");
    return { set: this.list("}", () => this.value()) };
  }
  record() {
    this.eat("[");
    const pairs = this.list("]", () => this.pair("|->"));
    return Object.fromEntries(pairs);
  }
  func() {
    this.eat("(");
    const pairs = [this.pair(":>")];
    while (this.peek("@@")) {
      this.eat("@@");
      pairs.push(this.pair(":>"));
    }
    this.eat(")");
    return Object.fromEntries(pairs);
  }
  pair(sep) {
    const k = this.value();
    this.eat(sep);
    return [String(k), this.value()];
  }
  str() {
    const m = this.s.slice(this.i).match(/^"((?:[^"\\]|\\.)*)"/);
    this.i += m[0].length;
    return m[1];
  }
  atom() {
    const m = this.s.slice(this.i).match(/^-?[A-Za-z0-9_]+/);
    if (!m) throw new Error(`bad atom at ${this.i}: ${this.s.slice(this.i, this.i + 40)}`);
    this.i += m[0].length;
    if (m[0] === "TRUE") return true;
    if (m[0] === "FALSE") return false;
    return /^-?\d+$/.test(m[0]) ? Number(m[0]) : m[0];
  }
}

function parseState(label) {
  const vars = {};
  for (const line of label.split(/\n(?=\/\\ )/)) {
    const m = line.match(/^\/\\ (\w+) = (.*)$/s);
    if (m && !DROP.has(m[1])) vars[m[1]] = new Parser(m[2]).value();
  }
  return vars;
}

function parseAction(label) {
  const m = label.match(/^(\w+)(?:\((.*)\))?$/s);
  if (!m) throw new Error(`bad action label ${label}`);
  return { name: m[1], params: m[2] === undefined ? [] : splitParams(m[2]) };
}

function splitParams(text) {
  const p = new Parser(`${text})`);
  return p.list(")", () => p.value());
}

function readGraph(dot) {
  const nodes = new Map();
  const edges = new Set();
  let init = null;
  for (const line of dot.split("\n")) {
    const edge = line.match(/^(-?\d+) -> (-?\d+) /);
    const node = line.match(/^(-?\d+) \[label=/);
    if (edge) edges.add([edge[1], edge[2], labelOf(line)].join("\u0000"));
    else if (node) {
      nodes.set(node[1], parseState(labelOf(line)));
      if (line.includes("style = filled")) init = node[1];
    }
  }
  return { nodes, edges, init };
}

function intern(table, index, value) {
  const key = JSON.stringify(value);
  if (!index.has(key)) {
    index.set(key, table.length);
    table.push(value);
  }
  return index.get(key);
}

function encode(job, graph) {
  const ids = [...graph.nodes.keys()];
  const pos = new Map(ids.map((id, i) => [id, i]));
  const vars = Object.keys(graph.nodes.get(graph.init)).sort();
  const values = Object.fromEntries(vars.map((v) => [v, []]));
  const vindex = Object.fromEntries(vars.map((v) => [v, new Map()]));
  const states = ids.map((id) => vars.map((v) => intern(values[v], vindex[v], graph.nodes.get(id)[v])));
  const actions = [];
  const aindex = new Map();
  const edges = [];
  for (const e of graph.edges) {
    const [from, to, label] = e.split("\u0000");
    if (/^(RL_Done|D_Done)$/.test(label)) continue;
    edges.push([pos.get(from), pos.get(to), intern(actions, aindex, parseAction(label))]);
  }
  edges.sort((a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2]);
  return { name: job.name, module: job.module, spec: specDigest(), constants: job.constants, init: pos.get(graph.init), vars, values, states, actions, edges };
}

function specDigest() {
  return Object.fromEntries(SPEC_FILES.map((f) => [f, createHash("sha256").update(readFileSync(join(SPEC_DIR, f))).digest("hex").slice(0, 16)]));
}

function exportJob(job) {
  const dir = mkdtempSync(join(tmpdir(), `orch-mbt-${job.name}-`));
  try {
    const fixture = encode(job, readGraph(runChecker(job, dir)));
    writeFileSync(join(OUT, `${job.name}.json`), JSON.stringify(fixture));
    return { name: job.name, states: fixture.states.length, edges: fixture.edges.length, actions: fixture.actions.length };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function main() {
  const wanted = process.argv.slice(2);
  mkdirSync(OUT, { recursive: true });
  for (const job of CONFIGS.filter((j) => wanted.length === 0 || wanted.includes(j.name))) {
    const row = exportJob(job);
    console.log(`${row.name}: ${row.states} states, ${row.edges} edges, ${row.actions} action labels`);
  }
}

main();
