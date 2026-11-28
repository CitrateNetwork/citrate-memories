# 1Password template for the memrizz Vercel project (team saulbuilds-projects,
# production). References only: no value is ever written to disk or put in argv.
# The names below are the names the code reads (webapp/src/lib/db/client.ts, webapp/drizzle.config.ts); the vault fields have the same names.
#
# On Vercel the Postgres variables are written by the Neon integration (the POSTGRES_*
# family is present), so these lines are the operator copy, not a push list. Do not
# overwrite them while the integration is attached (confirm in the Vercel Storage tab),
# and never point them at an archived branch. To push a line once the integration is
# detached (value on stdin; never `vercel env pull`):
#   op read --no-newline '<op ref>' | vercel env add <NAME> production --sensitive --force
# Local migrations against this database (value only in the child's environment):
#   op run --env-file <(grep -E '^[A-Z0-9_]+=' webapp/deploy/vercel.env.tpl | tr -d '{} ') -- node <migrate script>
#
# HOLD (PBA-L5-001): the memories database password rotation is coordinated. Do not push
# or rotate from this file until that item is closed.
DATABASE_URL={{ op://citrate-infra/memories-webapp.neon/DATABASE_URL }}
# The same item also holds DATABASE_URL_UNPOOLED, which the code does not read.
