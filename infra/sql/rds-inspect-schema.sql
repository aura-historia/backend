SELECT coalesce(jsonb_agg(relation ORDER BY relation->>'relation'), '[]'::jsonb)
FROM (
  SELECT jsonb_build_object(
    'relation', n.nspname || '.' || c.relname,
    'kind', c.relkind,
    'replica_identity', c.relreplident,
    'row_security', c.relrowsecurity,
    'columns', coalesce((
      SELECT jsonb_agg(jsonb_build_object(
        'name', a.attname, 'type', format_type(a.atttypid, a.atttypmod),
        'nullable', NOT a.attnotnull, 'generated', a.attgenerated,
        'default', pg_get_expr(d.adbin, d.adrelid)
      ) ORDER BY a.attnum)
      FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
      WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
    ), '[]'::jsonb),
    'constraints', coalesce((
      SELECT jsonb_agg(pg_get_constraintdef(k.oid) ORDER BY k.conname)
      FROM pg_constraint k WHERE k.conrelid = c.oid
    ), '[]'::jsonb),
    'indexes', coalesce((
      SELECT jsonb_agg(pg_get_indexdef(i.indexrelid) ORDER BY i.indexrelid::regclass::text)
      FROM pg_index i WHERE i.indrelid = c.oid
    ), '[]'::jsonb),
    'triggers', coalesce((
      SELECT jsonb_agg(pg_get_triggerdef(t.oid) ORDER BY t.tgname)
      FROM pg_trigger t WHERE t.tgrelid = c.oid AND NOT t.tgisinternal
    ), '[]'::jsonb)
  ) AS relation
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
    AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'
    AND c.relkind IN ('r', 'p', 'v', 'm') AND c.relname <> '_sqlx_migrations'
) relations;
