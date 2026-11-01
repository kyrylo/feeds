SELECT jsonb_build_object(
  'columns', (SELECT jsonb_agg(jsonb_build_array(c.relname,a.attname,format_type(a.atttypid,a.atttypmod),a.attnotnull,pg_get_expr(d.adbin,d.adrelid)) ORDER BY c.relname,a.attname)
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid
    LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
    WHERE n.nspname='public' AND c.relname=ANY(ARRAY['accounts','ar_internal_metadata','columns','events','folders','saved_views','schema_migrations','sessions','sink_memberships','sinks','users']) AND a.attnum>0 AND NOT a.attisdropped),
  'constraints', (SELECT jsonb_agg(jsonb_build_array(c.relname,k.conname,pg_get_constraintdef(k.oid)) ORDER BY c.relname,k.conname)
    FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND c.relname=ANY(ARRAY['accounts','ar_internal_metadata','columns','events','folders','saved_views','schema_migrations','sessions','sink_memberships','sinks','users']) AND k.contype IN ('p','f','u','c')),
  'indexes', (SELECT jsonb_agg(jsonb_build_array(t.relname,i.relname,pg_get_indexdef(i.oid),x.indisvalid,x.indisready) ORDER BY t.relname,i.relname)
    FROM pg_index x JOIN pg_class i ON i.oid=x.indexrelid JOIN pg_class t ON t.oid=x.indrelid JOIN pg_namespace n ON n.oid=t.relnamespace
    WHERE n.nspname='public' AND t.relname=ANY(ARRAY['accounts','ar_internal_metadata','columns','events','folders','saved_views','schema_migrations','sessions','sink_memberships','sinks','users'])),
  'extensions', (SELECT jsonb_agg(extname ORDER BY extname) FROM pg_extension WHERE extname='pg_trgm')
);
