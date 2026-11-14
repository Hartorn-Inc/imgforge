#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '..');
const ARCHIFY_BIN = path.join(REPO_ROOT, 'scripts', 'archify', 'bin', 'archify.mjs');
const OUTPUT_DIR = path.join(REPO_ROOT, 'docs', 'public', 'diagrams');

const DIAGRAMS = [
  {
    slug: 'imgforge-pipeline',
    type: 'architecture',
    input: 'docs/archify/imgforge-pipeline.architecture.json',
    output: 'docs/public/diagrams/imgforge-pipeline.html',
  },
  {
    slug: 'imgforge-lifecycle',
    type: 'lifecycle',
    input: 'docs/archify/imgforge-lifecycle.lifecycle.json',
    output: 'docs/public/diagrams/imgforge-lifecycle.html',
  },
  {
    slug: 'imgforge-editing-and-guides',
    type: 'workflow',
    input: 'docs/archify/imgforge-editing-and-guides.workflow.json',
    output: 'docs/public/diagrams/imgforge-editing-and-guides.html',
  },
];

function runArchify(args) {
  return spawnSync(process.execPath, [ARCHIFY_BIN, ...args], {
    cwd: REPO_ROOT,
    encoding: 'utf8',
  });
}

function parseJsonResult(res, label) {
  const raw = (res.stdout || '').trim();
  if (!raw) {
    throw new Error(
      `${label} produced no JSON output (exit ${res.status}): ${res.stderr || '(no stderr)'}`
    );
  }
  try {
    return JSON.parse(raw);
  } catch (err) {
    throw new Error(`${label} emitted invalid JSON: ${err.message}\n${raw}`);
  }
}

function validateDiagram(spec) {
  const res = runArchify([
    'validate',
    spec.type,
    spec.input,
    '--quality',
    'showcase',
    '--repo-root',
    REPO_ROOT,
    '--json',
  ]);
  const receipt = parseJsonResult(res, `validate ${spec.input}`);
  const status = receipt.composition?.status ?? receipt.status;
  const errors = receipt.composition?.summary?.errors ?? receipt.counts?.errors ?? -1;
  const warnings = receipt.composition?.summary?.warnings ?? receipt.counts?.warnings ?? -1;
  const ok = receipt.ok === true && status === 'pass' && errors === 0 && warnings === 0;
  if (res.status !== 0 || !ok) {
    throw new Error(
      `Validation failed for ${spec.input}: status=${status}, ok=${receipt.ok}, errors=${errors}, warnings=${warnings}\n${JSON.stringify(receipt.composition?.issues ?? receipt.diagnostics ?? receipt, null, 2)}`
    );
  }
  return receipt;
}

function checkHtml(spec) {
  const absOut = path.join(REPO_ROOT, spec.output);
  if (!existsSync(absOut)) {
    throw new Error(`Expected rendered HTML missing: ${spec.output}`);
  }
  const res = runArchify(['check', spec.output, '--json']);
  const receipt = parseJsonResult(res, `check ${spec.output}`);
  const status = receipt.composition?.status ?? receipt.status;
  const errors = receipt.composition?.summary?.errors ?? receipt.counts?.errors ?? -1;
  const warnings = receipt.composition?.summary?.warnings ?? receipt.counts?.warnings ?? -1;
  const checksOk = Array.isArray(receipt.checks) && receipt.checks.length > 0 && receipt.checks.every((c) => c.ok === true);
  if (res.status !== 0 || receipt.ok !== true || status !== 'pass' || errors !== 0 || warnings !== 0 || !checksOk) {
    throw new Error(
      `HTML check failed for ${spec.output}: status=${status}, ok=${receipt.ok}, errors=${errors}, warnings=${warnings}\n${JSON.stringify(receipt.checks ?? receipt.diagnostics ?? receipt, null, 2)}`
    );
  }
  return receipt;
}

function removeSidecars(spec) {
  const absOut = path.join(REPO_ROOT, spec.output);
  const parsed = path.parse(absOut);
  const stem = path.join(parsed.dir, parsed.name);
  const sidecars = [
    `${stem}.delivery-receipt.json`,
    `${stem}.delivery.json`,
    `${stem}.delivery-pending.json`,
    `${stem}.finalize.json`,
    `${stem}.finalize-summary.json`,
    `${stem}.browser-check.json`,
  ];
  for (const file of sidecars) {
    if (existsSync(file)) {
      rmSync(file, { force: true });
    }
  }
}

function renderDiagram(spec) {
  mkdirSync(OUTPUT_DIR, { recursive: true });
  const res = runArchify([
    'render',
    spec.type,
    spec.input,
    spec.output,
    '--quality',
    'showcase',
    '--repo-root',
    REPO_ROOT,
  ]);
  if (res.status !== 0) {
    throw new Error(
      `Render failed for ${spec.input} (exit ${res.status}): ${res.stderr || res.stdout}`
    );
  }
  removeSidecars(spec);
}

function main() {
  const args = new Set(process.argv.slice(2));
  const checkOnly = args.has('--check');

  if (!existsSync(ARCHIFY_BIN)) {
    throw new Error(`Archify CLI not found at ${ARCHIFY_BIN}`);
  }

  for (const spec of DIAGRAMS) {
    const valReceipt = validateDiagram(spec);
    if (!checkOnly) {
      renderDiagram(spec);
    }
    const chkReceipt = checkHtml(spec);
    const checksPassed = chkReceipt.checks.filter((c) => c.ok === true).length;
    const mode = checkOnly ? 'checked' : 'built+checked';
    console.log(
      `[archify] ${spec.slug} (${spec.type}): ${mode} — validate errors=0 warnings=0, check=pass (${checksPassed}/${chkReceipt.checks.length} checks)`
    );
    void valReceipt;
  }
}

try {
  main();
} catch (err) {
  console.error(`[archify] ERROR: ${err.message}`);
  process.exit(1);
}
