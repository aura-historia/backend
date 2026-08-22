<h1 align="center">Aura Historia — Backend</h1>

<p align="center">
  <strong>Backend powering the <a href="https://aura-historia.com">Aura Historia</a> art & antiques platform</strong>
</p>

<p align="center">
  <a href="https://aura-historia.com"><img src="https://img.shields.io/badge/aura--historia.com-Visit%20Website-8B4513?style=flat" alt="Website" /></a>
  &nbsp;
  <a href="https://docs.api.aura-historia.com/"><img src="https://img.shields.io/badge/OpenAPI-Docs-85EA2D?style=flat&logo=swagger&logoColor=white" alt="OpenAPI Docs" /></a>
</p>

<p align="center">
  <a href="https://github.com/aura-historia/backend/actions/workflows/cicd.yml"><img src="https://github.com/aura-historia/backend/actions/workflows/integrate.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/aura-historia/backend/actions/workflows/github-code-scanning/codeql"><img src="https://github.com/aura-historia/backend/actions/workflows/github-code-scanning/codeql/badge.svg" alt="CodeQL" /></a>
  <a href="https://github.com/aura-historia/backend/actions/workflows/dependabot/dependabot-updates"><img src="https://github.com/aura-historia/backend/actions/workflows/dependabot/dependabot-updates/badge.svg" alt="Dependabot" /></a>
</p>

<p align="center">
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=alert_status" alt="Quality Gate" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=coverage" alt="Coverage" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=ncloc" alt="Lines of Code" /></a>
</p>

<p align="center">
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=bugs" alt="Bugs" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=code_smells" alt="Code Smells" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=vulnerabilities" alt="Vulnerabilities" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=sqale_rating" alt="Maintainability" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=reliability_rating" alt="Reliability" /></a>
  <a href="https://sonarcloud.io/summary/new_code?id=aura-historia_backend"><img src="https://sonarcloud.io/api/project_badges/measure?project=aura-historia_backend&metric=security_rating" alt="Security" /></a>
</p>

---

## Overview

Aura Historia Backend is a Rust-based serverless application on AWS. 
It provides the APIs, event-driven pipelines, and data services that power the Aura Historia antiques marketplace — from product search and shop discovery to user management and real-time notifications.

## Development

```sh
# Check dependencies
cargo check --workspace

# Build all Lambda functions and binaries
cargo build --workspace

# Run unit tests
cargo test --workspace --lib --all-features

# Run integration tests (requires Localstack Ultimate/Enterprise/Student)
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export AWS_REGION=eu-central-1
export LOCALSTACK_AUTH_TOKEN=[your_localstack_pro_token]
cargo test --workspace --test integration --all-features

```

## License

[CC BY-NC-SA 4.0](LICENSE)

---

<p align="center">
  <img src="https://aura-historia-public.s3.eu-central-1.amazonaws.com/branding/banner_twitter_slogan.png" alt="Aura Historia — Where antiques find their story" />
</p>
