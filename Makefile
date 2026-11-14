SHELL := /bin/bash
.DEFAULT_GOAL := help

IMAGE_NAME ?= imgforge-builder:local
DOCKERFILE ?= Dockerfile
HOST_UID := $(shell id -u)
HOST_GID := $(shell id -g)

# Host-owned XDG cache directory so `docker run --user $(HOST_UID):$(HOST_GID)`
# never hits root-owned named volume permissions.
DOCKER_CACHE_DIR ?= $(if $(XDG_CACHE_HOME),$(XDG_CACHE_HOME)/imgforge,$(HOME)/.cache/imgforge)

DOCKER_RUN = \
	mkdir -p "$(DOCKER_CACHE_DIR)/cargo" "$(DOCKER_CACHE_DIR)/npm" && \
	docker run --rm \
		--cap-drop=ALL \
		--security-opt=no-new-privileges \
		--user "$(HOST_UID):$(HOST_GID)" \
		-v "$(CURDIR):/workspace" \
		-v "$(DOCKER_CACHE_DIR)/cargo:/cache/cargo" \
		-v "$(DOCKER_CACHE_DIR)/npm:/cache/npm" \
		-e CARGO_HOME=/cache/cargo \
		-e npm_config_cache=/cache/npm \
		-e HOME=/tmp \
		-w /workspace \
		"$(IMAGE_NAME)"

ENSURE_FRONTEND_DEPS = \
	if [ ! -d frontend/node_modules ] || { [ -f frontend/package-lock.json ] && { [ ! -f frontend/node_modules/.package-lock.json ] || [ frontend/package-lock.json -nt frontend/node_modules/.package-lock.json ]; }; }; then \
		if [ -f frontend/package-lock.json ]; then \
			npm --prefix frontend ci --ignore-scripts --prefer-offline; \
		else \
			npm --prefix frontend install --ignore-scripts; \
		fi; \
	fi

define ENSURE_IMAGE
	@docker image inspect "$(IMAGE_NAME)" >/dev/null 2>&1 || $(MAKE) docker-build
endef

.PHONY: help docker-build ui-build build test lint fmt-check fmt clean

help: ## Show available imgforge build and verification targets
	@printf "imgforge canonical workflow targets:\n\n"
	@awk 'BEGIN {FS = ":.*##"} /^[a-zA-Z0-9_-]+:.*##/ {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

docker-build: ## Build the pinned Rust + Node 22 LTS builder container image
	docker build \
		--build-arg BUILDER_UID="$(HOST_UID)" \
		--build-arg BUILDER_GID="$(HOST_GID)" \
		-t "$(IMAGE_NAME)" \
		-f "$(DOCKERFILE)" \
		.

ui-build: ## Install frontend dependencies and build the embedded Web Canvas SPA
	$(ENSURE_IMAGE)
	@$(DOCKER_RUN) bash -euo pipefail -c '\
		if [ -f frontend/package.json ]; then \
			$(ENSURE_FRONTEND_DEPS); \
			npm --prefix frontend run build; \
		else \
			echo "[imgforge] skipping ui-build: frontend/package.json not present yet"; \
		fi'

build: ui-build ## Build the embedded SPA and compile the release Rust binary for WSL2 host
	$(ENSURE_IMAGE)
	@$(DOCKER_RUN) bash -euo pipefail -c '\
		if [ -f Cargo.toml ]; then \
			if [ -f Cargo.lock ]; then \
				cargo build --release --locked; \
			else \
				cargo build --release; \
			fi; \
		else \
			echo "[imgforge] skipping Rust release build: Cargo.toml not present yet"; \
		fi'

test: ui-build ## Run frontend and Rust workspace test suites inside the builder container
	$(ENSURE_IMAGE)
	@$(DOCKER_RUN) bash -euo pipefail -c '\
		if [ -f frontend/package.json ]; then \
			$(ENSURE_FRONTEND_DEPS); \
			npm --prefix frontend test --if-present; \
		else \
			echo "[imgforge] skipping frontend tests: frontend/package.json not present yet"; \
		fi; \
		if [ -f Cargo.toml ]; then \
			if [ -f Cargo.lock ]; then \
				cargo test --workspace --locked; \
			else \
				cargo test --workspace; \
			fi; \
		else \
			echo "[imgforge] skipping Rust tests: Cargo.toml not present yet"; \
		fi'

lint: ui-build ## Run frontend typecheck/lint and cargo clippy (-D warnings)
	$(ENSURE_IMAGE)
	@$(DOCKER_RUN) bash -euo pipefail -c '\
		if [ -f frontend/package.json ]; then \
			$(ENSURE_FRONTEND_DEPS); \
			npm --prefix frontend run --if-present lint; \
			npm --prefix frontend run --if-present typecheck; \
		else \
			echo "[imgforge] skipping frontend lint: frontend/package.json not present yet"; \
		fi; \
		if [ -f Cargo.toml ]; then \
			if [ -f Cargo.lock ]; then \
				cargo clippy --workspace --all-targets --all-features --locked -- -D warnings; \
			else \
				cargo clippy --workspace --all-targets --all-features -- -D warnings; \
			fi; \
		else \
			echo "[imgforge] skipping Rust lint: Cargo.toml not present yet"; \
		fi'

fmt-check: ## Check formatting for frontend and Rust workspace without modifying files
	$(ENSURE_IMAGE)
	@$(DOCKER_RUN) bash -euo pipefail -c '\
		if [ -f frontend/package.json ]; then \
			$(ENSURE_FRONTEND_DEPS); \
			npm --prefix frontend run --if-present format:check; \
		else \
			echo "[imgforge] skipping frontend fmt-check: frontend/package.json not present yet"; \
		fi; \
		if [ -f Cargo.toml ]; then \
			cargo fmt --all -- --check; \
		else \
			echo "[imgforge] skipping Rust fmt-check: Cargo.toml not present yet"; \
		fi'

fmt: ## Format frontend and Rust source files inside the builder container
	$(ENSURE_IMAGE)
	@$(DOCKER_RUN) bash -euo pipefail -c '\
		if [ -f frontend/package.json ]; then \
			$(ENSURE_FRONTEND_DEPS); \
			npm --prefix frontend run --if-present format; \
		else \
			echo "[imgforge] skipping frontend fmt: frontend/package.json not present yet"; \
		fi; \
		if [ -f Cargo.toml ]; then \
			cargo fmt --all; \
		else \
			echo "[imgforge] skipping Rust fmt: Cargo.toml not present yet"; \
		fi'

clean: ## Remove compiled Rust and frontend build artifacts
	rm -rf target frontend/dist frontend/node_modules/.vite
