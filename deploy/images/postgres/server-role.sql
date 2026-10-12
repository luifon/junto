-- The role of the server and what it owns in one database (ADR 0022). psql runs this file as
-- the first user of the data directory, in the database that the role is to own:
--   - at the first start of a new data directory, as an init file of the PostgreSQL image;
--   - from deploy/database-user.sh, for a deployment of an earlier release, in which the
--     server connected as the first user and that user owns every object;
--   - from deploy/restore.sh, in the staged database, before the dump is loaded.
-- It can be run again: it changes only what differs.
--
-- The owner of a database can create an object with the name of a system object and can
-- change the search path of the database. This file runs as the first user, so it reads
-- names of the system schema only, from its first statement on.
set search_path = pg_catalog;
\getenv password POSTGRES_PASSWORD

select 'create role junto_server'
where not exists (select from pg_roles where rolname = 'junto_server') \gexec
-- The server copies its database before a migration, which needs CREATEDB and the ownership
-- of the database (ADR 0018). It needs no other attribute.
--
-- The statement holds the password. PostgreSQL writes a statement to its log when the
-- statement fails, and when the logging of statements is on; psql prints the line of a
-- statement that fails. The four settings turn that off for this statement.
\set VERBOSITY terse
set log_min_error_statement = 'panic';
set log_statement = 'none';
set log_min_duration_statement = -1;
alter role junto_server login createdb nosuperuser nocreaterole noreplication nobypassrls
    password :'password';
reset log_min_duration_statement;
reset log_statement;
reset log_min_error_statement;
\set VERBOSITY default

select format('alter database %I owner to junto_server', datname)
from pg_database
where datname = current_database() and datdba <> 'junto_server'::regrole \gexec

-- PostgreSQL refuses REASSIGN OWNED for the first user, so each object changes its owner:
-- the tables, views, sequences, routines, types and schemas that the first user owns and
-- that are not part of PostgreSQL or of an extension. Indexes and the sequences of columns
-- follow their tables.
select format('alter %s %s owner to junto_server', kind, name)
from (
    select case relation.relkind
               when 'v' then 'view'
               when 'm' then 'materialized view'
               when 'S' then 'sequence'
               when 'f' then 'foreign table'
               else 'table'
           end as kind,
           relation.oid::regclass::text as name,
           'pg_class'::regclass as catalog, relation.oid, relation.relnamespace as schema,
           relation.relowner as owner
    from pg_class relation
    where relation.relkind in ('r', 'p', 'v', 'm', 'S', 'f')
      and not (relation.relkind = 'S' and exists (
          select from pg_depend of_column
          where of_column.classid = 'pg_class'::regclass and of_column.objid = relation.oid
            and of_column.refclassid = 'pg_class'::regclass and of_column.refobjsubid > 0
            and of_column.deptype in ('a', 'i')))
    union all
    select 'routine', routine.oid::regprocedure::text,
           'pg_proc'::regclass, routine.oid, routine.pronamespace, routine.proowner
    from pg_proc routine
    union all
    select 'type', type.oid::regtype::text,
           'pg_type'::regclass, type.oid, type.typnamespace, type.typowner
    from pg_type type
    where type.typtype in ('e', 'd', 'r')
       or (type.typtype = 'c'
           and (select relkind from pg_class where pg_class.oid = type.typrelid) = 'c')
    union all
    select 'schema', quote_ident(schema.nspname),
           'pg_namespace'::regclass, schema.oid, schema.oid, schema.nspowner
    from pg_namespace schema
) object
where owner = (select oid from pg_roles where rolname = current_user)
  and (select nspname !~ '^pg_' and nspname <> 'information_schema'
       from pg_namespace where pg_namespace.oid = object.schema)
  and not exists (
      select from pg_depend of_extension
      where of_extension.classid = object.catalog and of_extension.objid = object.oid
        and of_extension.deptype = 'e')
order by kind, name \gexec
