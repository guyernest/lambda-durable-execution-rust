# Durable Lambda MCP Agent — Build & Deploy

PMCP_RUN_DIR := $(HOME)/Development/mcp/sdk/pmcp-run
AGENT_TARGET_DIR := $(PMCP_RUN_DIR)/amplify/functions/durable-agent-lambda/lambda-target

.PHONY: build test lint fmt check deploy-build clean

# Build the agent Lambda binary for ARM64 (Lambda target)
build:
	cargo lambda build --manifest-path examples/Cargo.toml --release --arm64 --bin mcp_agent

# Run all tests (SDK + examples)
test:
	cargo test
	cargo test --manifest-path examples/Cargo.toml --all-targets

# Lint with clippy
lint:
	cargo clippy --all-targets --all-features -D warnings

# Format check
fmt:
	cargo fmt --check

# Fast typecheck
check:
	cargo check
	cargo check --manifest-path examples/Cargo.toml --all-targets

# Build and copy agent binary to pmcp-run for Amplify deployment
deploy-build: build
	mkdir -p $(AGENT_TARGET_DIR)
	cp target/lambda/mcp_agent/bootstrap $(AGENT_TARGET_DIR)/bootstrap
	@echo "Agent binary copied to $(AGENT_TARGET_DIR)/bootstrap"

# Build with SAM (alternative deployment path)
sam-build:
	sam build -t examples/template-agent.yaml --beta-features

# Clean build artifacts
clean:
	cargo clean
