import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const TLA_TS = join(HERE, "..", "node_modules", ".bin", "tla-ts");
const TLC_JAR = process.env.TLC_JAR ?? join(HERE, "..", "tools", "tla2tools.jar");
const OUT = join(HERE, "Fast_out");
const CFG_DIR = join(OUT, "cfg");
const META_DIR = join("/tmp", `fast-tier-meta-${process.pid}`);
const JOB_CAP_MS = 2 * 60 * 1000;
const WORKERS = "4";
const SIM_TRACES = 200;
const SIM_DEPTH = 100;
const PASS = "PASS";

const SECTION_HEADERS = /^(SPECIFICATION|CONSTANTS?|INVARIANTS?|PROPERTIES|PROPERTY|SYMMETRY|INIT|NEXT|CHECK_DEADLOCK|VIEW|CONSTRAINTS?|ACTION_CONSTRAINTS?|ALIAS)\b/;

const bfs = (j) => ({ mode: "bfs", checkDeadlock: false, overrides: {}, expect: PASS, ...j });
const sim = (j) => ({ mode: "sim", checkDeadlock: false, overrides: {}, expect: PASS, ...j });

const PL_SMALL = { MaxSeq: 2, DupBudget: 0, ExpireBudget: 0, RetryBudget: 0, OfflineBudget: 0 };
const CT_SMALL = { MaxMsg: 4 };

const JOBS = [
  bfs({ name: "PL_Base", tag: "PL", module: "Placement", base: "MC_PL_Base", overrides: PL_SMALL, checkDeadlock: true }),
  bfs({ name: "PL_W_F11", tag: "PL", module: "Placement", base: "MC_PL_W_F11", overrides: PL_SMALL, expect: "Invariant F11Witness" }),
  bfs({ name: "PL_IndInv", tag: "PL", module: "PlacementInv", base: "MC_PL_IndInv", overrides: PL_SMALL }),
  bfs({ name: "PL_M_NoHostCheck", tag: "PL", module: "Placement", base: "MC_PL_M_NoHostCheck", overrides: PL_SMALL, expect: "Invariant S9" }),
  bfs({ name: "CT_Switch", tag: "CT", module: "ContextTransfer", base: "CT_Switch_N", overrides: CT_SMALL , checkDeadlock: true}),
  bfs({ name: "CT_Sub", tag: "CT", module: "ContextTransfer", base: "CT_Sub_N", overrides: CT_SMALL , checkDeadlock: true}),
  bfs({ name: "CT_Fork", tag: "CT", module: "ContextTransfer", base: "CT_Fork_N", overrides: CT_SMALL , checkDeadlock: true}),
  bfs({ name: "CT_Mut_NoDedup", tag: "CT", module: "ContextTransfer", base: "CT_Mut_NoDedup", overrides: CT_SMALL, expect: "Invariant NoDuplicate" }),
  bfs({ name: "NS_N", tag: "NS", module: "NativeSubagents", base: "NS_N", overrides: { MaxRuns: 1 } , checkDeadlock: true}),
  bfs({ name: "RL_Main", tag: "RL", module: "RunLifecycle", base: "MC_RL", overrides: { MaxPerson: 1 } , checkDeadlock: true}),
  bfs({ name: "Del_Main", tag: "Del", module: "Delegation", base: "MC_Del", overrides: { Threads: "{r, a}", MaxFanout: 1, MaxGen: 2 } , checkDeadlock: true}),
  bfs({ name: "Del_Q", tag: "Del", module: "Delegation", base: "MC_Del_Q", overrides: { MaxFail: 0, MaxCrash: 0 } , checkDeadlock: true}),
  bfs({ name: "Comp_Stub", tag: "Comp", module: "CompositionStub", base: "CompositionStub" , checkDeadlock: true}),
  bfs({ name: "ZI_Ref", tag: "ZI", module: "ZI_Refinement", base: "ZI_Ref", overrides: { MaxFail: 0 } }),
  bfs({ name: "ZI_M_NoDedupe", tag: "ZI", module: "ZI_Refinement", base: "ZI_Ref_M_NoDedupe", overrides: { MaxFail: 0 }, expect: "Action property" }),
  bfs({ name: "ZI_Dup", tag: "ZI", module: "ZI_Refinement", base: "ZI_Dup" }),
  bfs({ name: "ZI_Dup_M", tag: "ZI", module: "ZI_Refinement", base: "ZI_Dup_M_DupNoReceipt", expect: "Action property" }),
  bfs({ name: "ZI_Read_M", tag: "ZI", module: "ZI_Refinement", base: "ZI_Read_M_AckAtReply", expect: "Invariant ZI_AckObserved" }),
  bfs({ name: "ZI_Q_M", tag: "ZI", module: "ZI_Refinement", base: "ZI_Q_M_QNoReceipt", expect: "Action property" }),
  bfs({ name: "ZI_Acc_M", tag: "ZI", module: "ZI_Refinement", base: "ZI_Acc_M_NoClampSend", expect: "Invariant ZI_ClampInFlight" }),
  bfs({ name: "ZI_D2_W", tag: "ZI", module: "ZI_Refinement", base: "ZI_D2_W_OtherCrash", expect: "Invariant W_WaitSurvivesOtherCrash" }),
  bfs({ name: "ZI_PL_Dbg", tag: "ZI", module: "ZI_PlacementRef", base: "ZI_PL_Dbg" }),
  bfs({ name: "ZI_NS_Ref", tag: "ZI", module: "ZI_NSRef", base: "ZI_NS_Ref" }),
  bfs({ name: "ZI_CT_Switch", tag: "ZI", module: "ZI_CTRef", base: "ZI_CT_Switch" }),
  bfs({ name: "Comp_QuickAll", tag: "Comp", module: "Composition", base: "COMP_QuickAll" }),
  bfs({ name: "Del_Stop2", tag: "Del", module: "Delegation", base: "MC_Del_Stop2" }),
  sim({ name: "PL_N1_sim", tag: "PL", module: "Placement", base: "MC_PL_BaseN1" }),
  sim({ name: "CT_Sub_N1_sim", tag: "CT", module: "ContextTransfer", base: "CT_Sub_N1" }),
  sim({ name: "CT_Switch_N1_sim", tag: "CT", module: "ContextTransfer", base: "CT_Switch_N1" }),
  sim({ name: "CT_Fork_N1_sim", tag: "CT", module: "ContextTransfer", base: "CT_Fork_N1" }),
  sim({ name: "NS_N1_Runs_sim", tag: "NS", module: "NativeSubagents", base: "NS_N1_Runs" }),
  sim({ name: "NS_N1_Subs_sim", tag: "NS", module: "NativeSubagents", base: "NS_N1_Subs" }),
  sim({ name: "RL_N1_sim", tag: "RL", module: "RunLifecycle", base: "MC_RL_N1" }),
  sim({ name: "Del_N1_sim", tag: "Del", module: "Delegation", base: "MC_Del_N1" }),
  sim({ name: "Del_Q_N1_sim", tag: "Del", module: "Delegation", base: "MC_Del_Q_N1" }),
];

function applyOverrides(text, overrides, jobName) {
  let out = text;
  for (const [key, value] of Object.entries(overrides)) {
    const line = new RegExp(`^(\\s*)${key}\\s*=.*$`, "m");
    if (!line.test(out)) throw new Error(`${jobName}: constant ${key} not in base config`);
    out = out.replace(line, `$1${key} = ${value}`);
  }
  return out;
}

function dropSections(text, dropped) {
  const kept = [];
  let skipping = false;
  for (const line of text.split("\n")) {
    const header = line.match(SECTION_HEADERS);
    if (header) skipping = dropped.has(header[1]);
    if (!skipping) kept.push(line);
  }
  return kept.join("\n");
}

function writeConfig(job) {
  const base = readFileSync(join(HERE, `${job.base}.cfg`), "utf8");
  const sized = applyOverrides(base, job.overrides, job.name);
  const simSafe = job.mode === "sim" ? dropSections(sized, new Set(["PROPERTIES", "PROPERTY", "SYMMETRY"])) : sized;
  const header = `\\* Fast tier (Fast_run.mjs): ${job.name} from ${job.base}.cfg, overrides ${JSON.stringify(job.overrides)}. Expected: ${job.expect}\n`;
  const path = join(CFG_DIR, `Fast_${job.name}.cfg`);
  writeFileSync(path, header + simSafe);
  return path;
}

function commandFor(job, cfgPath) {
  const deadlock = job.checkDeadlock ? [] : ["-deadlock"];
  if (job.mode === "bfs") return [TLA_TS, ["-workers", WORKERS, ...deadlock, "-config", cfgPath, `${job.module}.tla`]];
  return ["java", ["-Xmx2g", "-XX:+UseParallelGC", "-cp", TLC_JAR, "tlc2.TLC",
    "-simulate", `num=${SIM_TRACES}`, "-depth", String(SIM_DEPTH), "-workers", WORKERS, ...deadlock,
    "-metadir", join(META_DIR, job.name), "-noGenerateSpecTE", "-config", cfgPath, `${job.module}.tla`]];
}

function classify(log) {
  if (/No error has been found/.test(log)) return PASS;
  const inv = log.match(/Invariant (\w+) is violated/);
  if (inv) return `Invariant ${inv[1]}`;
  if (/Action property .* is violated/.test(log)) return "Action property";
  if (/Temporal propert(y|ies) .*violated/.test(log)) return "Temporal";
  if (/Deadlock reached/.test(log)) return "Deadlock";
  const simulationFinished = /Running Random Simulation/.test(log) && /Finished in/.test(log) && !/^Error:/m.test(log);
  if (simulationFinished) return PASS;
  return "ERROR";
}

function distinctStates(log) {
  const done = [...log.matchAll(/([\d,]+) states generated, ([\d,]+) distinct states found/g)].at(-1);
  if (done) return done[2].replaceAll(",", "");
  const traces = log.match(/(\d[\d,]*) states checked/);
  return traces ? `${traces[1].replaceAll(",", "")} checked` : "-";
}

function outcomeOf(job, result, timedOut) {
  if (timedOut) return "TIMEOUT";
  if (result === "ERROR") return "ERROR";
  const expectsPass = job.expect === PASS;
  if (expectsPass) return result === PASS ? "PASS" : "UNEXPECTED-FAIL";
  if (result === PASS) return "MISSED (expected fail not reached at these bounds)";
  return result.startsWith(job.expect) ? "EXPECTED-FAIL" : "WRONG-FAIL";
}

function runJob(job) {
  const cfgPath = writeConfig(job);
  const [cmd, args] = commandFor(job, cfgPath);
  const started = Date.now();
  const r = spawnSync("nice", ["-n", "10", cmd, ...args], {
    cwd: HERE, encoding: "utf8", maxBuffer: 1 << 28, timeout: JOB_CAP_MS, killSignal: "SIGKILL",
  });
  const secs = Math.round((Date.now() - started) / 1000);
  const log = `${r.stdout ?? ""}${r.stderr ?? ""}`;
  writeFileSync(join(OUT, `${job.name}.log`), log);
  const timedOut = r.error?.code === "ETIMEDOUT" || r.signal === "SIGKILL";
  const classified = classify(log);
  const result = timedOut ? "-" : (classified === PASS && r.status !== 0 ? "ERROR" : classified);
  return { name: job.name, tag: job.tag, mode: job.mode, expected: job.expect, result,
    outcome: outcomeOf(job, result, timedOut), distinct: distinctStates(log), secs };
}

const OK_OUTCOMES = new Set(["PASS", "EXPECTED-FAIL"]);

function table(rows) {
  const head = "| Job | Mode | Outcome | Result | Expected | States | Secs |\n|---|---|---|---|---|---|---|";
  const body = rows.map((r) => `| ${r.name} | ${r.mode} | ${r.outcome} | ${r.result} | ${r.expected} | ${r.distinct} | ${r.secs} |`);
  return [head, ...body].join("\n");
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes("--list")) {
    for (const j of JOBS) console.log(`${j.tag}\t${j.name}\t${j.mode}\t${j.base}\t${j.expect}`);
    return;
  }
  const wanted = args.filter((a) => !a.startsWith("--"));
  const selected = JOBS.filter((j) => wanted.length === 0 || wanted.includes(j.tag) || wanted.includes(j.name));
  if (selected.length === 0) throw new Error("No matching model jobs");
  mkdirSync(CFG_DIR, { recursive: true });
  mkdirSync(META_DIR, { recursive: true });
  const rows = [];
  for (const job of selected) {
    const row = runJob(job);
    rows.push(row);
    console.log(`${row.outcome.padEnd(16)} ${row.name} (${row.secs}s, ${row.result})`);
  }
  const md = table(rows);
  writeFileSync(join(OUT, "results.json"), JSON.stringify(rows, null, 2));
  writeFileSync(join(OUT, "results.md"), `${md}\n`);
  console.log(`\n${md}`);
  const failed = rows.filter((r) => !OK_OUTCOMES.has(r.outcome));
  process.exitCode = failed.length === 0 ? 0 : 1;
}

main();
