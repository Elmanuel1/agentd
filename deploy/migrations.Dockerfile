# agentd's schema migrations in Flyway 11, built and versioned with the agentd image. The Helm chart
# (deploy/helm/agentd) runs it as a Job before each install or upgrade rolls agentd out.
#   docker build -f deploy/migrations.Dockerfile -t agentd-migrations:<tag> .
FROM flyway/flyway:11-alpine
COPY crates/adapters/store-postgres/migrations /flyway/sql
ENV FLYWAY_LOCATIONS=filesystem:/flyway/sql
