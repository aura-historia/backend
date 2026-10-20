'use strict';
const { command, hash } = require('./io.cjs');
const git = (args) => command('git', ['--no-pager', ...args]);
function ancestor(from, to, run = git) {
  try {
    run(['merge-base', '--is-ancestor', from, to]);
    return true;
  } catch {
    return false;
  }
}
function files(ref, directory, run) {
  return run(['ls-tree', '-r', '--name-only', ref, '--', directory]).trim().split('\n').filter(Boolean);
}
function blob(ref, file, run) {
  const line = run(['ls-tree', ref, '--', file]).trim();
  return line ? line.split(/\s+/)[2] : null;
}
function declarations(baseline, target, run = git) {
  const previous = new Set(files(baseline, 'releases/requirements', run));
  const current = files(target, 'releases/requirements', run).filter((file) => file.endsWith('.json'));
  // An intermediate removal/edit must not erase a requirement, even if a later commit restores it.
  const altered = run([
    'log',
    '--full-history',
    '--no-renames',
    '--format=',
    '--name-only',
    '--diff-filter=MD',
    `${baseline}..${target}`,
    '--',
    'releases/requirements',
  ]).trim();
  if (altered || [...previous].some((file) => !files(target, 'releases/requirements', run).includes(file)))
    throw new Error(
      'Release requirement declarations are append-only. Restore the declaration and use a dedicated procedure for changed requirements.',
    );
  return current
    .filter((file) => !previous.has(file))
    .map((file) => {
      const entry = JSON.parse(run(['show', `${target}:${file}`]));
      if (
        entry.version !== 1 ||
        !/^[a-z0-9][a-z0-9-]{2,100}$/.test(entry.id || '') ||
        file !== `releases/requirements/${entry.id}.json` ||
        !['OPENSEARCH', 'COMPATIBILITY'].includes(entry.kind) ||
        !['NONE', 'MANUAL', 'UNSUPPORTED'].includes(entry.classification) ||
        typeof entry.reason !== 'string' ||
        !entry.reason.trim() ||
        !/^docs\/[\w./-]+\.md(?:#[\w-]+)?$/.test(entry.runbook || '')
      )
        throw new Error(`Invalid release requirement ${file}; see docs/infra.md#release-requirements.`);
      if (entry.classification === 'UNSUPPORTED')
        throw new Error(
          `Requirement ${entry.id} needs a dedicated operation: ${entry.runbook}. Routine release is blocked.`,
        );
      if (
        entry.kind === 'COMPATIBILITY' &&
        (entry.classification !== 'NONE' ||
          entry.runningApplicationCompatible !== true ||
          entry.retainedWorkCompatible !== true)
      )
        throw new Error(
          `Requirement ${entry.id} cannot run with existing application traffic and retained work. Use ${entry.runbook}.`,
        );
      if (
        entry.classification === 'MANUAL' &&
        (entry.kind !== 'OPENSEARCH' ||
          typeof entry.action !== 'string' ||
          !entry.action.trim() ||
          typeof entry.validation !== 'string' ||
          !entry.validation.trim())
      )
        throw new Error(
          `Manual requirement ${entry.id} must describe compatible OpenSearch preparation and validation.`,
        );
      if (
        entry.requiresDeployed !== undefined &&
        (!/^[a-f0-9]{40}$/.test(entry.requiresDeployed) || !ancestor(entry.requiresDeployed, baseline, run))
      )
        throw new Error(
          `Requirement ${entry.id} needs deployed prerequisite ${entry.requiresDeployed} before migration. Release that prerequisite first; see ${entry.runbook}.`,
        );
      return entry;
    });
}
function coverage(changes, entries, baseline, target, run) {
  for (const file of changes) {
    let position = blob(baseline, file, run);
    const expected = blob(target, file, run);
    const links = entries.flatMap((entry) => (entry.assets?.[file] ? [entry.assets[file]] : []));
    const seen = new Set();
    while (position !== expected) {
      const candidates = links.filter(
        (link) => link.from === position && (link.to === null || /^[a-f0-9]{40}$/.test(link.to || '')),
      );
      if (candidates.length !== 1 || seen.has(position))
        throw new Error(
          `Unclassified or ambiguous change to ${file}. Add an append-only release requirement with exact Git blob from/to hashes.`,
        );
      seen.add(position);
      position = candidates[0].to;
    }
  }
}
function requirements(baseline, target, run = git) {
  if (!ancestor(baseline, target, run))
    throw new Error(
      'Routine releases must move forward from the completed release. Use the guarded exceptional rollback procedure.',
    );
  const entries = declarations(baseline, target, run);
  if (new Set(entries.map((e) => e.id)).size !== entries.length)
    throw new Error('Duplicate release requirement identity.');
  const changed = (path) =>
    run(['diff', '--name-only', baseline, target, '--', path]).trim().split('\n').filter(Boolean);
  const search = changed('opensearch');
  const sql = changed('migrations').filter((file) => file.endsWith('.sql'));
  coverage(
    search,
    entries.filter((e) => e.kind === 'OPENSEARCH'),
    baseline,
    target,
    run,
  );
  coverage(
    sql,
    entries.filter((e) => e.kind === 'COMPATIBILITY'),
    baseline,
    target,
    run,
  );
  const manual = entries.filter((e) => e.classification === 'MANUAL');
  return { id: hash({ baseline, target, entries }), entries, manual, search, sql };
}
module.exports = { requirements, declarations, coverage, ancestor, git };
