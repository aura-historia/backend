'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { randomUUID } = require('node:crypto');
const { run, ledgerFindings } = require('./inspection.cjs');
const SCHEMA_SQL = fs.readFileSync(path.join(__dirname, '../../infra/sql/rds-inspect-schema.sql'), 'utf8');
const POSTGRES_IMAGE = 'postgres:16-bookworm@sha256:efedf3595f1d6f415c08568ba171029bf54052e754cc9f030e3f2412b21f3d67';
async function snapshots(migrations, ledger, options = {}) {
  if (ledgerFindings(ledger, migrations).length) throw new Error('Cannot reconstruct unknown/edited/dirty migration history');
  const execute = options.run || run, name = `aura-review-${randomUUID()}`;
  const docker = args => execute('docker', args, { timeout: 180000 });
  docker(['run', '-d', '--rm', '--network', 'none', '--name', name, '-e', 'POSTGRES_HOST_AUTH_METHOD=trust', POSTGRES_IMAGE]);
  try {
    let ready = false;
    for (let attempt = 0; attempt < 40; attempt++) {
      try { docker(['exec', name, 'pg_isready', '-h', '127.0.0.1', '-U', 'postgres']); ready = true; break; }
      catch { await new Promise(resolve => setTimeout(resolve, 250)); }
    }
    if (!ready) throw new Error('Isolated schema PostgreSQL did not become ready');
    const sql = (database, input) => execute('docker', ['exec', '-i', name, 'psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1', '-h', '127.0.0.1', '-U', 'postgres', '-d', database],
      { input, stdio: ['pipe', 'pipe', 'pipe'], timeout: 180000 });
    sql('postgres', 'CREATE ROLE aura_runtime; CREATE ROLE aura_replication; CREATE ROLE aura_migrator; CREATE DATABASE before_schema; CREATE DATABASE after_schema;');
    const applied = new Set(ledger.map(row => row.version));
    for (const database of ['before_schema', 'after_schema']) for (const migration of migrations) {
      if (database === 'before_schema' && !applied.has(migration.version)) continue;
      if (!Number.isSafeInteger(migration.version) || typeof migration.sql !== 'string') throw new Error('Complete migration SQL required');
      sql(database, `SET statement_timeout='120s'; SET lock_timeout='5s';\n${migration.sql}`);
    }
    return { before: JSON.parse(sql('before_schema', SCHEMA_SQL)), after: JSON.parse(sql('after_schema', SCHEMA_SQL)), image: POSTGRES_IMAGE };
  } finally { docker(['rm', '-f', name]); }
}
module.exports = { SCHEMA_SQL, POSTGRES_IMAGE, snapshots };
