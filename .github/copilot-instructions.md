# Blitzfilter AWS Backend

Blitzfilter AWS Backend is a Rust-based serverless application that provides product management, shop discovery, user management, and notification services for AWS. The system consists of multiple Lambda functions, API Gateway handlers, and supporting services that work with DynamoDB, OpenSearch, SQS, SES, and Cognito.

Always reference these instructions first and fallback to search or bash commands only when you encounter unexpected information that does not match the info here.

## Working Effectively

### Bootstrap and Build
- Install Rust toolchain: `rustup install stable && rustup default stable`
- Bootstrap the workspace: `cd /home/runner/work/backend/backend`
- **NEVER CANCEL**: Check dependencies: `cargo check --workspace` -- takes 3 minutes on first run with downloads. Set timeout to 10+ minutes.
- **NEVER CANCEL**: Build workspace: `cargo build --workspace` -- takes 4-5 minutes on first run. Set timeout to 10+ minutes.

### Lint and Format
- Format check: `cargo fmt --all -- --check` -- takes 0.5 seconds
- Lint check: `cargo clippy --workspace --all-targets --all-features -- -D warnings` -- takes 15 seconds

### Testing
- **Unit tests**: `cargo test --workspace --lib --all-features` -- takes 35 seconds. Set timeout to 2+ minutes. Apply parameterized testing with crate `rstest` when plausible, e.g. for serialization.
- **Integration tests**: Require additional setup (see Prerequisites section)
- All test names need to follow a consistent naming convention, e.g., `should_[expectation]_when_[condition]_for_[purpose]`. The placeholders can be replaced with many words, e.g. `should_serialize_data_when_valid_for_storing`. Provide meaningful test-names that describe the purpose of the test.
- Most types have an instance for `fake::Dummy<fake::Faker>`. Our internal crates provide this functionality via feature-flag `test-data`. You may need to include it for dev-dependencies. Use it to generate test data when plausible.

### Prerequisites for Full Integration Testing
**WARNING**: These require network access and may fail in restricted environments:
- Install Zig: `npm install -g @ziglang/cli` -- may fail due to network restrictions
- **NEVER CANCEL**: Install cargo-lambda: `cargo install cargo-lambda` -- takes 15+ minutes. Set timeout to 30+ minutes.
- Docker must be available for LocalStack containers

## Validation

### Always Validate Changes
- **ALWAYS** run format and lint checks before committing: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
- **ALWAYS** run unit tests after code changes: `cargo test --workspace --lib --all-features`
- Run integration tests when changing core functionality: `cargo test --workspace --all-features --test '*'`

### Manual Testing Scenarios
Since this is a serverless backend, manual testing involves:
1. **Build validation**: Ensure all Lambda functions compile: `cargo build --workspace`
2. **Unit test validation**: Verify business logic: `cargo test --workspace --lib --all-features`
3. **Integration test validation**: Test AWS service integration with LocalStack containers
4. **CI validation**: The CI/CD pipeline (.github/workflows/cicd.yml) runs the complete test suite

### Limitations
- **Cannot run Lambda functions locally** without cargo-lambda and proper AWS setup
- **OpenSearch tests often timeout** in CI environments (5+ minutes, often fail)
- **Full integration testing requires network access** for tool installation
- **No CLI applications** - all components are Lambda functions or libraries

## Critical Timing and Timeout Information

**NEVER CANCEL these commands - they are expected to take significant time:**
- `cargo check --workspace`: 3 minutes (first run with downloads) - Set timeout to 10+ minutes
- `cargo build --workspace`: 4-5 minutes on first run - Set timeout to 10+ minutes
- `cargo install cargo-lambda`: 15+ minutes - Set timeout to 30+ minutes
- DynamoDB integration tests: 41 seconds - Set timeout to 2+ minutes
- OpenSearch integration tests: 5+ minutes (often timeout) - Set timeout to 10+ minutes
- Unit tests: 35 seconds - Set timeout to 2+ minutes

## Project Structure

### Key Modules
- **src/common**: Shared types, API utilities, error handling, pagination, currency, language, price models
- **src/product**: Core product management system with multiple sub-modules:
  - `core`: Core business logic and domain models for products
  - `data`: Data transfer objects and API models
  - `dynamodb`: Data access layer for products in DynamoDB
  - `opensearch`: Data access layer for products in OpenSearch
  - `service`: Business services for product operations
  - `watchlist`: User product watchlist functionality
- **src/product-api**: API Gateway handlers for product operations (8 handlers)
- **src/product-lambda**: Lambda function implementations for product event processing (6 lambdas)
- **src/product-enrichment**: Product data enrichment pipeline and auto-scaling
- **src/shop**: Shop/store management system:
  - `core`: Shop domain models and business logic
  - `data`: Shop data models
  - `dynamodb`: Shop data access layer for DynamoDB
  - `opensearch`: Shop data access layer for OpenSearch
  - `service`: Shop business services
- **src/shop-api**: API Gateway handlers for shop operations (2 handlers)
- **src/search-filter**: User search filter management:
  - `core`: Search filter domain models
  - `data`: Search filter data models
  - `dynamodb`: Search filter data access layer
  - `service`: Search filter business services
- **src/search-filter-api**: API Gateway handlers for search filter operations (5 handlers)
- **src/user**: User management system:
  - `core`: User domain models
  - `dynamodb`: User data access layer
  - `service`: User business services
- **src/cognito**: AWS Cognito integration utilities
- **src/cognito-post-confirmation**: Lambda for Cognito post-confirmation trigger
- **src/mail**: Email notification system:
  - `mail-core`: Email templates and core logic
- **src/mail-lambda-send**: Lambda for sending emails via SES
- **src/test-api**: Testing utilities and integration test framework
- **src/aws-tests**: AWS integration and end-to-end tests:
  - `aws-tests-common`: Common test utilities
  - `smoking-tests`: Basic smoke tests for deployed environments
  - `staging-tests`: Staging environment integration tests
  - `staging-data`: Test data for staging environment

### Lambda Functions (Executables)
Located in various directories:

**Product Lambda Functions** (`src/product-lambda/src/`):
- `product-lambda-ingest-events-dynamodb`: Ingest product events from DynamoDB streams
- `product-lambda-materialize-dynamodb-new`: Materialize new products to DynamoDB
- `product-lambda-materialize-dynamodb-update`: Materialize product updates to DynamoDB
- `product-lambda-materialize-opensearch-new`: Materialize new products to OpenSearch
- `product-lambda-materialize-opensearch-update`: Materialize product updates to OpenSearch
- `product-lambda-update-notify-user`: Notify users about product updates

**Product Enrichment Lambda Functions** (`src/product-enrichment/src/`):
- `product-enrichment-asg-scale-up`: Scale up Auto Scaling Group for enrichment
- `product-enrichment-asg-scale-down`: Scale down Auto Scaling Group after enrichment

**Cognito Lambda Functions**:
- `cognito-post-confirmation`: Handle Cognito user post-confirmation trigger (`src/cognito-post-confirmation`)

**Mail Lambda Functions**:
- `mail-lambda-send`: Send emails via AWS SES (`src/mail-lambda-send`)

### API Handlers
**Product API Handlers** (`src/product-api/src/`):
- `product-api-get-product`: Retrieve a single product by ID
- `product-api-get-product-similar`: Get similar products
- `product-api-search`: Search products with filters
- `product-api-put-products`: Bulk update/create products
- `product-api-watchlist-get`: Get user's product watchlist
- `product-api-watchlist-post`: Add product to watchlist
- `product-api-watchlist-patch`: Update watchlist entry
- `product-api-watchlist-delete`: Remove product from watchlist

**Shop API Handlers** (`src/shop-api/src/`):
- `shop-api-get-shop`: Retrieve shop details by ID
- `shop-api-search`: Search shops

**Search Filter API Handlers** (`src/search-filter-api/src/`):
- `search-filter-api-get-search-filter`: Get a single search filter
- `search-filter-api-get-search-filters`: List user's search filters
- `search-filter-api-post-search-filter`: Create a new search filter
- `search-filter-api-patch-search-filter`: Update a search filter
- `search-filter-api-delete-search-filter`: Delete a search filter

## Common Tasks

### Building Specific Components
- Build all Lambda functions: `cargo build --workspace`
- Build specific Lambda: `cd src/product-lambda/src/product-lambda-materialize-dynamodb-new && cargo build --all-features`
- Build API handlers: `cd src/product-api/src/product-api-get-product && cargo build --all-features`
- Build specific module: `cd src/product && cargo build --all-features`

### Testing Specific Components
- Test core logic: `cd src/product && cargo test --lib --all-features`
- Test API layer: `cd src/product-api/src/product-api-get-product && cargo test --lib --all-features`
- Test shop module: `cd src/shop && cargo test --lib --all-features`
- Test search filters: `cd src/search-filter && cargo test --lib --all-features`
- Test user module: `cd src/user && cargo test --lib --all-features`
- **Integration tests**: `cd src/product && cargo test --test '*' --all-features` (requires LocalStack containers)

### Code Quality
- Format all code: `cargo fmt --all`
- Check formatting: `cargo fmt --all -- --check`
- Lint all code: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Frequently Referenced Files

### Workspace Configuration
```
Cargo.toml (workspace root)
├── Dependencies and workspace member definitions
└── Shared dependency versions across all crates
```

### Main Source Directories
```
src/
├── common/          # Shared utilities, API types, error handling, currency, language, price
├── product/         # Product management system (main business logic)
│   ├── src/core/           # Domain models and business rules for products
│   ├── src/data/           # Product data transfer objects
│   ├── src/dynamodb/       # Product data access for DynamoDB
│   ├── src/opensearch/     # Product search and indexing
│   ├── src/service/        # Product business services
│   └── src/watchlist/      # User product watchlist
├── product-api/     # API Gateway handlers for products (8 handlers)
│   └── src/         # Individual API handler crates
├── product-lambda/  # Lambda function implementations (6 lambdas)
│   └── src/         # Individual lambda crates
├── product-enrichment/  # Product enrichment pipeline
│   ├── python/      # Python enrichment scripts
│   └── src/         # Lambda functions for auto-scaling
├── shop/           # Shop/store management system
│   ├── src/core/           # Shop domain models
│   ├── src/data/           # Shop data models
│   ├── src/dynamodb/       # Shop data access for DynamoDB
│   ├── src/opensearch/     # Shop search and indexing
│   └── src/service/        # Shop business services
├── shop-api/       # API Gateway handlers for shops (2 handlers)
│   └── src/        # Individual API handler crates
├── search-filter/  # User search filter management
│   ├── src/core/           # Search filter domain models
│   ├── src/data/           # Search filter data models
│   ├── src/dynamodb/       # Search filter data access
│   └── src/service/        # Search filter business services
├── search-filter-api/  # API Gateway handlers for search filters (5 handlers)
│   └── src/        # Individual API handler crates
├── user/           # User management system
│   ├── src/core/           # User domain models
│   ├── src/dynamodb/       # User data access
│   └── src/service/        # User business services
├── cognito/        # AWS Cognito integration utilities
├── cognito-post-confirmation/  # Cognito post-confirmation Lambda
├── mail/           # Email notification system
│   └── src/mail-core/      # Email templates and core logic
├── mail-lambda-send/  # Lambda for sending emails via SES
├── test-api/       # Testing framework and integration test utilities
└── aws-tests/      # AWS integration and end-to-end tests
    └── src/
        ├── aws-tests-common/  # Common test utilities
        ├── smoking-tests/     # Basic smoke tests
        ├── staging-tests/     # Staging integration tests
        └── staging-data/      # Test data for staging
```

### CI/CD Configuration
- `.github/workflows/cicd.yml`: Complete CI/CD pipeline with lint, build, test, and deploy phases
- `.github/workflows/delete_pr_cfn_stack.yml`: CloudFormation stack cleanup for closed PRs
- `sonar-project.properties`: SonarQube configuration for code quality analysis

## Troubleshooting

### Build Issues
- **"cargo command not found"**: Install Rust toolchain: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Network timeouts during install**: Expected - retry with longer timeouts
- **Integration tests fail**: Ensure Docker is available and running

### Test Issues
- **Lambda tests fail**: Requires cargo-lambda installation (`cargo install cargo-lambda`)
- **OpenSearch tests timeout**: Expected in CI environments - focus on unit tests
- **DynamoDB tests slow**: Normal - uses Docker containers for LocalStack

### Performance
- **First build is slow**: Expected - downloads all dependencies (~4-5 minutes)
- **Subsequent builds are faster**: Rust incremental compilation works well
- **Integration tests are slow**: LocalStack container startup adds overhead
