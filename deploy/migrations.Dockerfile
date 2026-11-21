# agentd's schema migrations in Flyway 11, built and versioned with the agentd image. The Helm chart
# (deploy/helm/agentd) runs it as a Job before each install or upgrade rolls agentd out.
#   docker build -f deploy/migrations.Dockerfile -t agentd-migrations:<tag> .
FROM flyway/flyway:11.20.3-alpine@sha256:07f65a04ac518f3bf1c00a52dab46aa9649d931510860c6ac5363d6fb75e4c1b
COPY crates/adapters/store-postgres/migrations /flyway/sql
ENV FLYWAY_LOCATIONS=filesystem:/flyway/sql
