'use strict';

// Read-only structural acceptance check. Uses Node/Git only; never runs agent or backend code.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { TextDecoder } = require('node:util');

const NAMES = Object.freeze([
  'aura-maintenance', 'aura-architecture', 'aura-bug-hunt', 'aura-security',
  'aura-data-events', 'aura-contracts', 'aura-test-quality', 'aura-cleanup',
  'aura-operations', 'aura-verify',
]);
const PREFIX = '.agents/skills/';
const REFERENCES = ['policy.md', 'runbook.md', 'repo-map.md', 'report-template.md',
  'sources.md', 'smoke-tests.md'];
const FILES = Object.freeze([
  ...NAMES.flatMap(name => [`${PREFIX}${name}/SKILL.md`, `${PREFIX}${name}/agents/openai.yaml`]),
  ...REFERENCES.map(name => `${PREFIX}aura-maintenance/references/${name}`),
  `${PREFIX}aura-maintenance/scripts/verify.cjs`,
  `${PREFIX}aura-maintenance/scripts/verify.test.cjs`,
]);
const ALIASES = Object.freeze(NAMES.map(name => `.claude/skills/${name}`));
const REPO_PATHS = Object.freeze([
  'AGENTS.md', 'Cargo.toml', 'rust-toolchain.toml', 'depgraph-rules.toml',
  'infra/package.json', '.github/workflows/', 'ci/', 'migrations/', 'infra/',
  'docs/README.md', 'docs/arch.md', 'docs/storage.md', 'docs/product-listing.md',
  'docs/object-ids.md', 'docs/events/flow.md', 'docs/durable-worker-runbook.md',
  'docs/marketing-consent.md', 'docs/http-api-front-door.md', 'docs/swagger.yaml',
  'docs/CHANGELOG.md', 'docs/infra.md', 'docs/opensearch-stage.md',
  'docs/crawler/README.md', 'src/crawler/', 'src/public-network-policy/', 'src/image-fetcher/',
]);
const DEFAULT_ROOT = path.resolve(__dirname, '../../../..');
const MANUAL_POLICY = 'policy:\n  allow_implicit_invocation: false\n';

function git(root, args) {
  const result = spawnSync('git', ['--no-optional-locks', '-c', 'core.fsmonitor=false', '-C', root, ...args], {
    encoding: 'utf8', timeout: 10000, maxBuffer: 16 * 1024 * 1024,
    env: { ...process.env, GIT_TERMINAL_PROMPT: '0' },
  });
  if (result.error || result.status !== 0) {
    throw new Error(`Git ${args[0]} failed: ${result.error?.message || result.stderr.trim() || result.status}`);
  }
  return result.stdout;
}

function within(parent, child) {
  const relative = path.relative(parent, child);
  return relative === '' || (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`));
}

function parseFrontmatter(text) {
  const match = /^---\n([\s\S]*?)\n---\n/.exec(text);
  if (!match) throw new Error('missing or unclosed LF frontmatter');
  const values = Object.create(null);
  for (const line of match[1].split('\n')) {
    const field = /^([a-z][a-z-]*): (.+)$/.exec(line);
    if (!field) throw new Error('use one scalar field per line');
    const [, key, raw] = field;
    if (!['name', 'description', 'disable-model-invocation'].includes(key)) {
      throw new Error(`unsupported frontmatter field ${key}`);
    }
    if (Object.hasOwn(values, key)) throw new Error(`duplicate frontmatter field ${key}`);
    if (key === 'description') {
      values[key] = JSON.parse(raw); // JSON double-quoted strings are valid YAML scalars.
    } else values[key] = raw;
  }
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(values.name || '')) throw new Error('invalid skill name');
  if (typeof values.description !== 'string' || values.description.length < 40 || values.description.length > 1024) {
    throw new Error('description must be a quoted string of 40-1024 characters');
  }
  if (values['disable-model-invocation'] !== 'true') throw new Error('Claude manual invocation must be true');
  return { ...values, body: text.slice(match[0].length) };
}

function verify(root, { base } = {}) {
  root = path.resolve(root);
  const errors = [];
  const contents = new Map();
  const expected = new Set([...FILES, ...ALIASES]);
  const fail = message => errors.push(message);
  const attempt = (label, operation) => {
    try { return operation(); } catch (error) { fail(`${label}: ${error.message}`); return undefined; }
  };
  const realRoot = attempt('repository root', () => fs.realpathSync(root));
  if (!realRoot) return errors;

  // Do not follow unexpected symlinks while enumerating or reading canonical files.
  function walk(relative) {
    attempt(relative, () => {
      const absolute = path.join(root, relative);
      const stat = fs.lstatSync(absolute);
      if (stat.isDirectory()) {
        for (const name of fs.readdirSync(absolute)) walk(`${relative}/${name}`);
      } else if (stat.isFile() && expected.has(relative)) {
        return;
      } else fail(`${relative}: unexpected file/type in canonical pack`);
    });
  }
  for (const directory of ['.agents', '.agents/skills', '.claude', '.claude/skills']) {
    attempt(directory, () => {
      if (!fs.lstatSync(path.join(root, directory)).isDirectory()) throw new Error('must be a real directory');
    });
  }
  if (errors.length) return errors;
  for (const name of NAMES) walk(`${PREFIX}${name}`);
  for (const clientRoot of ['.agents/skills', '.claude/skills']) {
    attempt(clientRoot, () => {
      for (const name of fs.readdirSync(path.join(root, clientRoot))) {
        if (name.startsWith('aura-') && !NAMES.includes(name)) fail(`${clientRoot}/${name}: unexpected Aura skill`);
      }
    });
  }

  for (const relative of FILES) {
    attempt(relative, () => {
      const absolute = path.join(root, relative);
      if (!fs.lstatSync(absolute).isFile()) throw new Error('must be a regular file');
      // Check every parent, not only the leaf; a parent symlink is also disallowed.
      for (let current = path.dirname(absolute); current !== root; current = path.dirname(current)) {
        if (!fs.lstatSync(current).isDirectory()) throw new Error('canonical parent must not be a symlink');
      }
      if (!within(realRoot, fs.realpathSync(absolute))) throw new Error('path escapes repository');
      const bytes = fs.readFileSync(absolute);
      contents.set(relative, bytes);
      const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      if (!text.endsWith('\n') || text.includes('\r')) throw new Error('use LF and a final newline');
      if (!relative.endsWith('.md')) return;
      if (relative.endsWith('/SKILL.md')) {
        const metadata = parseFrontmatter(text);
        if (metadata.name !== relative.split('/')[2]) throw new Error('name must match directory');
        if (metadata.body.trim().split(/\s+/).length < 200 || text.split('\n').length > 500) {
          throw new Error('stub or oversized skill body (require 200+ words and at most 500 lines)');
        }
        if (!text.includes('AGENTS.md') || !text.includes('mode=audit') || !text.includes('execution=static')) {
          throw new Error('missing repository guidance or safe defaults');
        }
        if (!text.includes('references/policy.md)') || !text.includes('references/runbook.md)')) {
          throw new Error('missing shared policy/runbook links');
        }
        if (/!`|\$ARGUMENTS|\bTODO\b|\bTBD\b|insert (?:skill|instructions) here/i.test(text)) {
          throw new Error('placeholder or dynamic command interpolation is not permitted');
        }
      }
      // Pack Markdown deliberately uses only inline, whitespace-free link destinations.
      for (const match of text.matchAll(/\[[^\]\n]+\]\(([^)]+)\)/g)) {
        const href = match[1];
        if (/\s/.test(href)) throw new Error(`unsupported whitespace in link: ${href}`);
        if (href.startsWith('https://')) { new URL(href); continue; }
        if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('/')) throw new Error(`unsafe link ${href}`);
        const [file, fragment] = href.split('#');
        const target = path.resolve(path.dirname(absolute), decodeURIComponent(file));
        if (!within(path.join(root, PREFIX), target) || !within(path.join(realRoot, PREFIX), fs.realpathSync(target))) {
          throw new Error(`local link escapes canonical skills: ${href}`);
        }
        if (!fs.statSync(target).isFile()) throw new Error(`link is not a file: ${href}`);
        if (fragment) throw new Error(`use a file link without an unchecked anchor: ${href}`);
      }
    });
  }

  for (const name of NAMES) {
    const metadata = `${PREFIX}${name}/agents/openai.yaml`;
    if (contents.get(metadata)?.toString('utf8') !== MANUAL_POLICY) fail(`${metadata}: Codex manual policy mismatch`);
    const relative = `.claude/skills/${name}`;
    attempt(relative, () => {
      const absolute = path.join(root, relative);
      if (!fs.lstatSync(absolute).isSymbolicLink()) throw new Error('must be a Git directory symlink, not a copied file');
      const target = fs.readlinkSync(absolute);
      if (target !== `../../.agents/skills/${name}`) throw new Error('wrong relative symlink target');
      if (fs.realpathSync(absolute) !== fs.realpathSync(path.join(root, PREFIX, name))) throw new Error('alias target mismatch');
      contents.set(relative, Buffer.from(target));
    });
  }

  attempt('Git index', () => {
    if (fs.realpathSync(git(root, ['rev-parse', '--show-toplevel']).trim()) !== realRoot) {
      throw new Error('root must be the Git working-tree root');
    }
    const tracked = new Map();
    for (const line of git(root, ['ls-files', '--stage', '-z']).split('\0').filter(Boolean)) {
      const match = /^(\d{6}) ([0-9a-f]+) (\d)\t([\s\S]+)$/.exec(line);
      if (!match || match[3] !== '0') throw new Error('invalid or conflicted index');
      tracked.set(match[4], { mode: match[1], hash: match[2] });
    }
    for (const relative of tracked.keys()) {
      const owned = NAMES.some(name => relative.startsWith(`${PREFIX}${name}/`)) ||
        ALIASES.some(alias => relative === alias || relative.startsWith(`${alias}/`));
      if (owned && !expected.has(relative)) fail(`${relative}: unexpected tracked pack entry`);
    }
    const algorithm = git(root, ['rev-parse', '--show-object-format']).trim();
    if (!['sha1', 'sha256'].includes(algorithm)) throw new Error('unsupported Git object format');
    for (const relative of expected) {
      const entry = tracked.get(relative);
      const mode = ALIASES.includes(relative) ? '120000' : '100644';
      if (!entry || entry.mode !== mode) { fail(`${relative}: untracked or wrong Git mode (expected ${mode})`); continue; }
      const bytes = contents.get(relative);
      if (bytes) {
        const hash = crypto.createHash(algorithm).update(`blob ${bytes.length}\0`).update(bytes).digest('hex');
        if (hash !== entry.hash) fail(`${relative}: working content differs from Git index`);
      }
    }
    for (const required of REPO_PATHS) {
      const exists = required.endsWith('/') ? [...tracked.keys()].some(file => file.startsWith(required)) : tracked.has(required);
      if (!exists) fail(`${required}: repository reference missing from Git index`);
    }
    if (base !== undefined) {
      const resolved = git(root, ['rev-parse', '--verify', '--end-of-options', `${base}^{commit}`]).trim();
      git(root, ['merge-base', '--is-ancestor', resolved, 'HEAD']);
      for (const relative of git(root, ['diff', '--no-ext-diff', '--name-only', '-z', resolved, '--']).split('\0').filter(Boolean)) {
        if (!expected.has(relative)) fail(`${relative}: change outside skill-only scope`);
      }
    }
  });
  return errors;
}

function main(args) {
  let root = DEFAULT_ROOT;
  let base;
  const seen = new Set();
  for (let i = 0; i < args.length; i++) {
    const key = args[i];
    if (!['--root', '--base'].includes(key) || seen.has(key) || !args[i + 1]) {
      console.error('Usage: node verify.cjs [--root REPOSITORY] [--base COMMIT]');
      return 2;
    }
    seen.add(key);
    if (key === '--root') root = args[++i]; else base = args[++i];
  }
  const errors = verify(root, { base });
  if (errors.length) {
    console.error(`FAIL: ${errors.length} structural problem(s)\n${errors.map(error => `- ${error}`).join('\n')}`);
    return 1;
  }
  console.log(`PASS: ${NAMES.length} canonical skills, ${NAMES.length} tracked Claude aliases, metadata and local references.`);
  console.log('Structural checks only. Agent behavior, backend checks and client smoke tests are NOT certified.');
  return 0;
}

module.exports = { verify, parseFrontmatter, NAMES, FILES, ALIASES, REPO_PATHS, DEFAULT_ROOT, main };
if (require.main === module) process.exitCode = main(process.argv.slice(2));
