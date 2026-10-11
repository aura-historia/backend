'use strict';

// Deliberately synthetic Git repositories. These test the checker, not model behavior.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync, spawnSync } = require('node:child_process');
const { verify, parseFrontmatter, NAMES, FILES, ALIASES, REPO_PATHS, DEFAULT_ROOT } = require('./verify.cjs');
const SKILL = '.agents/skills/aura-security/SKILL.md';
const ALIAS = '.claude/skills/aura-security';
const SCRIPT = '.agents/skills/aura-maintenance/scripts/verify.cjs';

function runGit(root, ...args) {
  return execFileSync('git', ['-C', root, '-c', 'user.name=Skill Fixture',
    '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false',
    '-c', 'core.fsmonitor=false', ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}
function write(root, relative, content) {
  const file = path.join(root, relative);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
}
function fixture(t) {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'aura-skill-test-'));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const root = path.join(temp, 'repo');
  fs.mkdirSync(root);
  runGit(root, 'init', '--quiet');
  runGit(root, 'config', 'core.hooksPath', path.join(temp, 'no-hooks'));
  for (const required of REPO_PATHS) {
    write(root, required.endsWith('/') ? `${required}_fixture` : required, 'Synthetic path-existence fixture only.\n');
  }
  runGit(root, 'add', '.');
  runGit(root, 'commit', '--quiet', '-m', 'synthetic baseline');
  const base = runGit(root, 'rev-parse', 'HEAD').trim();
  for (const relative of FILES) write(root, relative, fs.readFileSync(path.join(DEFAULT_ROOT, relative)));
  fs.mkdirSync(path.join(root, '.claude/skills'), { recursive: true });
  for (const name of NAMES) fs.symlinkSync(`../../.agents/skills/${name}`, path.join(root, '.claude/skills', name), 'dir');
  runGit(root, 'add', '.agents', '.claude');
  return { root, temp, base };
}
function edit(root, relative, transform) {
  const file = path.join(root, relative);
  fs.writeFileSync(file, transform(fs.readFileSync(file, 'utf8')));
}
function fails(root, pattern, options) {
  assert.match(verify(root, options).join('\n'), pattern);
}

 test('valid pack and tracked symlink modes pass', t => {
  const { root, base } = fixture(t);
  assert.deepEqual(verify(root, { base }), []);
  assert.equal(ALIASES.length, 10);
  assert.match(runGit(root, 'ls-files', '--stage', ALIAS), /^120000 /);
});

test('fresh local clone preserves aliases and passes without installation', t => {
  const { root, temp } = fixture(t);
  runGit(root, 'commit', '--quiet', '-m', 'skills');
  const clone = path.join(temp, 'fresh');
  execFileSync('git', ['-c', 'core.symlinks=true', 'clone', '--quiet', '--no-hardlinks', root, clone]);
  assert.deepEqual(verify(clone), []);
  for (const alias of ALIASES) assert.equal(fs.lstatSync(path.join(clone, alias)).isSymbolicLink(), true);
});

test('validator is read-only', t => {
  const { root } = fixture(t);
  const before = runGit(root, 'status', '--porcelain=v1', '-z', '--untracked-files=all');
  const index = fs.readFileSync(path.join(root, '.git/index'));
  assert.deepEqual(verify(root), []);
  assert.equal(runGit(root, 'status', '--porcelain=v1', '-z', '--untracked-files=all'), before);
  assert.deepEqual(fs.readFileSync(path.join(root, '.git/index')), index);
});

test('missing skill is a failure', t => {
  const { root } = fixture(t);
  fs.unlinkSync(path.join(root, SKILL));
  fails(root, /aura-security\/SKILL.md/);
});

test('placeholder-sized specialist fails even with valid frontmatter', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => `${text.split('\n---\n')[0]}\n---\n\nRead the code.\n`);
  fails(root, /stub or oversized/);
});

test('wrong or duplicate canonical name is rejected', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => text.replace('name: aura-security', 'name: aura-cleanup'));
  fails(root, /name must match directory/);
});

test('duplicate frontmatter field is rejected', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => text.replace('name: aura-security', 'name: aura-security\nname: aura-security'));
  fails(root, /duplicate frontmatter/);
});

test('unknown permission frontmatter is rejected', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => text.replace('disable-model-invocation: true', 'disable-model-invocation: true\nallowed-tools: Bash'));
  fails(root, /unsupported frontmatter/);
});

test('malformed quoted description fails', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => text.replace('description: "', 'description: {'));
  assert.ok(verify(root).length > 0);
});

test('missing/invalid YAML delimiters fail', () => {
  for (const text of ['name: test\n', '---\nname: test\n', '\uFEFF---\nname: test\n---\n']) {
    assert.throws(() => parseFrontmatter(text), /frontmatter/);
  }
});

test('Claude automatic invocation cannot be enabled unnoticed', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => text.replace('disable-model-invocation: true', 'disable-model-invocation: false'));
  fails(root, /Claude manual invocation/);
});

test('Codex automatic invocation cannot be enabled unnoticed', t => {
  const { root } = fixture(t);
  edit(root, '.agents/skills/aura-security/agents/openai.yaml', text => text.replace('false', 'true'));
  fails(root, /Codex manual policy mismatch/);
});

test('broken reference fails', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => `${text}\n[missing](missing.md)\n`);
  fails(root, /missing.md/);
});

test('link with unsupported whitespace cannot silently bypass checking', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => `${text}\n[missing](missing file.md)\n`);
  fails(root, /unsupported whitespace/);
});

test('escaping local link fails', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => `${text}\n[escape](../../../AGENTS.md)\n`);
  fails(root, /aura-security\/SKILL.md/);
});

test('shared policy cannot be replaced by a symlink', t => {
  const { root, temp } = fixture(t);
  const relative = '.agents/skills/aura-maintenance/references/policy.md';
  fs.writeFileSync(path.join(temp, 'outside.md'), 'outside\n');
  fs.unlinkSync(path.join(root, relative));
  fs.symlinkSync(path.join(temp, 'outside.md'), path.join(root, relative));
  fails(root, /must be a regular file/);
});

test('canonical parent cannot be replaced by a symlink', t => {
  const { root, temp } = fixture(t);
  fs.renameSync(path.join(root, '.agents'), path.join(temp, 'outside-agents'));
  fs.symlinkSync(path.join(temp, 'outside-agents'), path.join(root, '.agents'), 'dir');
  fails(root, /must be a real directory/);
});

test('plain-text Windows-style alias is not accepted as a working Claude skill', t => {
  const { root } = fixture(t);
  fs.unlinkSync(path.join(root, ALIAS));
  fs.writeFileSync(path.join(root, ALIAS), '../../.agents/skills/aura-security');
  fails(root, /must be a Git directory symlink/);
});

test('wrong and cyclic aliases fail without recursion', t => {
  const { root } = fixture(t);
  fs.unlinkSync(path.join(root, ALIAS));
  fs.symlinkSync('aura-security', path.join(root, ALIAS));
  fails(root, /wrong relative symlink target/);
});

test('untracked alias fails', t => {
  const { root } = fixture(t);
  runGit(root, 'rm', '--cached', '--quiet', '--force', ALIAS);
  fails(root, /untracked or wrong Git mode/);
});

test('working-tree changes must be staged rather than silently omitted from the PR', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => `${text}\nAdditional local note.\n`);
  fails(root, /working content differs from Git index/);
});

test('unexpected installer or launcher fails', t => {
  const { root } = fixture(t);
  write(root, '.agents/skills/aura-maintenance/scripts/install.sh', 'echo unexpected\n');
  fails(root, /unexpected file\/type/);
});

test('deleted but still tracked unexpected file also fails', t => {
  const { root } = fixture(t);
  const extra = '.agents/skills/aura-maintenance/scripts/extra.sh';
  write(root, extra, 'unexpected\n');
  runGit(root, 'add', extra);
  fs.unlinkSync(path.join(root, extra));
  fails(root, /unexpected tracked pack entry/);
});

test('extra Aura skill cannot be silently omitted from the manifest', t => {
  const { root } = fixture(t);
  write(root, '.agents/skills/aura-extra/SKILL.md', 'unexpected\n');
  fails(root, /unexpected Aura skill/);
});

test('missing repository owner path fails', t => {
  const { root } = fixture(t);
  runGit(root, 'rm', '--quiet', 'docs/storage.md');
  fails(root, /docs\/storage.md: repository reference missing/);
});

test('invalid UTF-8 fails', t => {
  const { root } = fixture(t);
  fs.appendFileSync(path.join(root, SKILL), Buffer.from([0xff]));
  fails(root, /encoded data was not valid/);
});

test('CRLF and missing final newline fail', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => text.replaceAll('\n', '\r\n'));
  fails(root, /use LF and a final newline/);
});

test('dynamic shell expansion in a manifest fails', t => {
  const { root } = fixture(t);
  edit(root, SKILL, text => `${text}\n!\`echo unwanted\`\n`);
  fails(root, /dynamic command interpolation/);
});

test('scope guard rejects runtime changes', t => {
  const { root, base } = fixture(t);
  edit(root, 'Cargo.toml', text => `${text}changed\n`);
  fails(root, /change outside skill-only scope/, { base });
});

test('invalid comparison base is a failure, not a fallback', t => {
  const { root } = fixture(t);
  fails(root, /Git rev-parse failed/, { base: 'missing-ref' });
});

test('CLI reports structural PASS, never a client or backend test pass', t => {
  const { root } = fixture(t);
  const result = spawnSync(process.execPath, [path.join(root, SCRIPT), '--root', root], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /PASS: 10 canonical skills/);
  assert.match(result.stdout, /client smoke tests are NOT certified/);
});

test('CLI unknown options and duplicate options fail', t => {
  const { root } = fixture(t);
  for (const args of [['--install'], ['--root'], ['--root', root, '--root', root]]) {
    const result = spawnSync(process.execPath, [path.join(root, SCRIPT), ...args], { encoding: 'utf8' });
    assert.equal(result.status, 2);
  }
});

test('missing Git capability is an explicit failure', t => {
  const { root } = fixture(t);
  const result = spawnSync(process.execPath, [path.join(root, SCRIPT), '--root', root], {
    encoding: 'utf8', env: { ...process.env, PATH: '' },
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Git .* failed/);
});
