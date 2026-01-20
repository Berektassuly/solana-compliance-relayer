# Contributing to Solana Compliance Relayer

Thank you for your interest in contributing to the Solana Compliance Relayer. This document provides guidelines and requirements for contributing to a project that handles high-security financial operations on the Solana blockchain.

---

## Table of Contents

1. [Introduction and Philosophy](#introduction-and-philosophy)
2. [Getting Started](#getting-started)
3. [Workflow and Branching Strategy](#workflow-and-branching-strategy)
4. [Coding Standards](#coding-standards)
5. [Testing Requirements](#testing-requirements)
6. [Pull Request Checklist](#pull-request-checklist)
7. [Security Considerations](#security-considerations)

---

## Introduction and Philosophy

### Mission

The Solana Compliance Relayer is a mission-critical bridge between Solana and regulatory compliance protocols. Our priorities, in order, are:

1. **Security**: Every line of code must assume adversarial conditions
2. **Auditability**: All transactions must be traceable and verifiable
3. **Performance**: High throughput without compromising the above
4. **Maintainability**: Clean, documented code that others can understand

### Commit Convention

We follow [Conventional Commits](https://www.conventionalcommits.org/) for clear versioning and changelog generation:

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

**Types**:
| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation only |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `test` | Adding or correcting tests |
| `chore` | Maintenance tasks (dependencies, CI, etc.) |
| `security` | Security-related changes (use for any auth/crypto changes) |

**Examples**:
```
feat(worker): add exponential backoff for retry logic
fix(compliance): handle Range API timeout gracefully
security(auth): strengthen webhook signature validation
docs(readme): update deployment instructions
```

---

## Getting Started

### Repository Structure

This project follows a **multi-repository** architecture:

| Repository | Purpose |
|------------|---------|
| `solana-compliance-relayer` | Rust backend (Axum, SQLx, blockchain integration) |
| `solana-compliance-relayer-frontend` | Next.js frontend with WASM signer |

### Backend Development Environment

#### Prerequisites

- **Rust**: 1.75+ (stable)
- **Docker**: For PostgreSQL
- **sqlx-cli**: For migrations

#### Setup

```bash
# Clone the repository
git clone https://github.com/Berektassuly/solana-compliance-relayer.git
cd solana-compliance-relayer

# Install sqlx-cli
cargo install sqlx-cli --no-default-features --features postgres

# Start PostgreSQL
docker-compose up -d

# Copy environment template
cp .env.example .env
# Edit .env with your configuration

# Run migrations
cargo sqlx migrate run

# Verify setup
cargo build
cargo test
```

#### Recommended Tools

| Tool | Purpose | Installation |
|------|---------|--------------|
| `cargo-watch` | Auto-rebuild on changes | `cargo install cargo-watch` |
| `bacon` | Background code checker | `cargo install bacon` |
| `cargo-tarpaulin` | Code coverage | `cargo install cargo-tarpaulin` |

Run continuous checking during development:

```bash
bacon clippy
```

### Frontend Development Environment

The frontend lives in a separate repository:

```bash
git clone https://github.com/Berektassuly/solana-compliance-relayer-frontend.git
cd solana-compliance-relayer-frontend

# Install dependencies
pnpm install

# Start development server
pnpm run dev
```

#### WASM Signer Development

If modifying the WASM cryptographic module:

```bash
# Install wasm-pack
curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

# Add WASM target
rustup target add wasm32-unknown-unknown

# Rebuild WASM module
cd wasm-signer
wasm-pack build --target web --out-dir ../src/lib/wasm-pkg

# Clear Next.js cache and restart
rm -rf ../.next
cd ..
pnpm run dev
```

---

## Workflow and Branching Strategy

### Fork and Pull Request Model

1. **Fork** the repository to your GitHub account
2. **Clone** your fork locally
3. **Create a branch** from `main` for your changes
4. **Push** to your fork
5. **Open a Pull Request** against the upstream `main` branch

### Branch Naming Convention

```
<type>/<issue-number>-<short-description>
```

| Type | Use Case |
|------|----------|
| `feat/` | New features |
| `fix/` | Bug fixes |
| `docs/` | Documentation updates |
| `refactor/` | Code restructuring |
| `security/` | Security-related changes |
| `test/` | Test additions or fixes |

**Examples**:
```
feat/42-add-quicknode-priority-fees
fix/57-webhook-timeout-handling
security/61-rate-limit-bypass
```

### Pull Request Process

#### PR Title

Use the same Conventional Commits format:

```
feat(compliance): add Range Protocol v2 support
```

#### PR Description Template

```markdown
## Summary

Brief description of what this PR accomplishes.

## Related Issues

Fixes #42
Relates to #38

## Changes Made

- Added X to handle Y
- Refactored Z for better performance
- Updated documentation for W

## Testing

Describe how you tested these changes:
- [ ] Unit tests added/updated
- [ ] Integration tests pass
- [ ] Manual testing performed

## Security Considerations

Does this PR affect:
- [ ] Authentication/Authorization
- [ ] Cryptographic operations
- [ ] User input handling
- [ ] Database queries

If yes, describe the security implications and mitigations.

## Checklist

See [Pull Request Checklist](#pull-request-checklist) below.
```

---

## Coding Standards

### Rust: The Golden Rules

#### Error Handling

**Never use `panic!`, `unwrap()`, or `expect()` in production code paths.**

```rust
// BAD - Will crash the server
let value = some_option.unwrap();
let result = fallible_operation().expect("this should work");

// GOOD - Propagate errors gracefully
let value = some_option.ok_or_else(|| AppError::NotFound("Value missing".into()))?;
let result = fallible_operation().map_err(|e| AppError::Internal(e.to_string()))?;
```

**Exception**: `unwrap()` is acceptable in:
- Test code
- Compile-time verified constants (e.g., `Regex::new(r"...").unwrap()` in `lazy_static`)
- Infallible conversions where the type system cannot prove it

#### Error Type Guidelines

| Context | Error Type | Crate |
|---------|------------|-------|
| Application logic | `AppError` | `thiserror` |
| Library code | Custom error enums | `thiserror` |
| One-off scripts/main | `anyhow::Result` | `anyhow` |

#### Formatting and Linting

Run before every commit:

```bash
# Format code
cargo fmt

# Run linter with strict settings
cargo clippy -- -D warnings

# Verify no warnings
cargo build 2>&1 | grep -i warning && echo "FIX WARNINGS" || echo "OK"
```

#### Code Style

- Use `tracing` for logging, not `println!` or `log`
- Instrument async functions with `#[instrument(skip(self))]`
- Document public APIs with `///` doc comments
- Prefer explicit types over `impl Trait` in public APIs

### TypeScript/React Guidelines

For the frontend repository:

#### Component Structure

- Use **functional components** with hooks
- Prefer named exports over default exports
- Colocate component, styles, and tests

#### Type Safety

```typescript
// BAD - Avoid 'any'
function processData(data: any): any { ... }

// GOOD - Explicit types
interface TransferRequest {
  id: string;
  fromAddress: string;
  toAddress: string;
  amount: bigint;
}

function processData(data: TransferRequest): ProcessedResult { ... }
```

### WASM Cryptographic Code

Changes to cryptographic logic require extra scrutiny:

1. **Mirror in CLI**: Any signing logic change must be reflected in `src/bin/generate_transfer_request.rs`
2. **Cross-Verification**: Test that signatures generated by WASM match backend verification
3. **Document Algorithms**: Explicitly comment which cryptographic primitives are used

```rust
// Example: Document the signing scheme
/// Signs a message using Ed25519 with the following format:
/// Message: "{from_address}:{to_address}:{amount}:{token_mint|SOL}"
/// Signature: 64-byte Ed25519 signature, Base58 encoded
pub fn sign_transfer_request(...) -> String { ... }
```

---

## Testing Requirements

### Coverage Expectations

All PRs must include tests. The minimum coverage target is **80%** for new code.

### Test Categories

#### Unit Tests

For domain logic, state transitions, and pure functions:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compliance_status_transition() {
        let status = ComplianceStatus::Pending;
        assert!(status.can_transition_to(ComplianceStatus::Approved));
        assert!(!status.can_transition_to(ComplianceStatus::Pending));
    }
}
```

#### Integration Tests

For API handlers and database interactions:

```rust
#[sqlx::test]
async fn test_submit_transfer_persists_to_db(pool: PgPool) {
    let client = PostgresClient::from_pool(pool);
    let request = create_test_request();
    
    let result = client.submit_transfer(&request).await.unwrap();
    
    assert_eq!(result.compliance_status, ComplianceStatus::Pending);
}
```

#### WASM Verification Tests

Ensure browser-generated signatures match backend expectations:

```rust
#[test]
fn test_signature_verification_matches_wasm() {
    // Use the same keypair and message as WASM tests
    let message = "sender:recipient:1000000000:SOL";
    let signature = sign_with_known_key(message);
    
    let request = SubmitTransferRequest {
        from_address: KNOWN_PUBKEY.to_string(),
        signature: signature,
        // ...
    };
    
    assert!(request.verify_signature().is_ok());
}
```

### Running Tests

```bash
# Run all tests
cargo test

# Run with coverage report
cargo tarpaulin --out Html --output-dir coverage

# Run specific test
cargo test test_compliance_status_transition

# Run integration tests (requires Docker)
docker-compose up -d
cargo test --test integration_tests
```

---

## Pull Request Checklist

Copy this checklist into your PR description:

```markdown
## Checklist

### Code Quality
- [ ] Code follows project style guidelines
- [ ] No `unwrap()` or `panic!()` in production paths
- [ ] `cargo fmt` has been run
- [ ] `cargo clippy -- -D warnings` passes with no warnings

### Testing
- [ ] `cargo test` passes
- [ ] New code has unit tests
- [ ] Integration tests updated if API changed

### Documentation
- [ ] Public APIs have doc comments
- [ ] README updated (if user-facing changes)
- [ ] OpenAPI/Swagger annotations updated (if API changed)

### Security (if applicable)
- [ ] No secrets or credentials in code
- [ ] Input validation added for new endpoints
- [ ] SQL queries use parameterized inputs

### WASM (if modified)
- [ ] WASM module rebuilt
- [ ] CLI tools (`generate_transfer_request`) updated to match
- [ ] Cross-verification test added

### Commits
- [ ] Conventional Commits format used
- [ ] Commits are atomic and logical
- [ ] No merge commits (rebase instead)
```

---

## Security Considerations

### Reporting Vulnerabilities

**Do not open public issues for security vulnerabilities.**

Email security concerns to: [mukhammedali@berektassuly.com](mailto:mukhammedali@berektassuly.com)

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

### Sensitive Areas

The following areas require additional review from maintainers:

| Area | Reason |
|------|--------|
| `src/domain/types.rs` (signature verification) | Core authentication logic |
| `src/infra/compliance/` | Sanctions/AML screening |
| `src/api/handlers.rs` (webhook handler) | External input processing |
| `src/infra/blockchain/` | Transaction signing and submission |
| Any cryptographic code | Requires expert review |

PRs touching these areas will be held for security review before merging.

---

## Questions?

- Open a [Discussion](https://github.com/Berektassuly/solana-compliance-relayer/discussions) for general questions
- Check existing [Issues](https://github.com/Berektassuly/solana-compliance-relayer/issues) before opening new ones
- Review the [Technical Operations Guide](docs/GUIDE.md) for architecture details

Thank you for contributing to a more secure and compliant Solana ecosystem.
