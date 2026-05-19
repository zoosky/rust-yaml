# Makefile for rust-yaml project
# Consolidates development scripts and common commands

.PHONY: help setup clean test lint format doc bench audit coverage release check-all dev-setup test-yaml-suite test-yaml-suite-regen

# Default target
help: ## Show this help message
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

# Development Environment Setup
setup: ## Set up development environment (git hooks, dependencies, etc.)
	@echo "🔧 Setting up development environment..."
	@if [ ! -d ".githooks" ]; then mkdir -p .githooks; fi
	@if [ -f ".githooks/commit-msg" ]; then chmod +x .githooks/commit-msg; fi
	@echo "🔗 Configuring git hooks..."
	@git config --local core.hooksPath .githooks
	@if [ -f ".gitmessage" ]; then \
		echo "📋 Setting commit message template..."; \
		git config --local commit.template .gitmessage; \
	fi
	@if ! command -v committed >/dev/null 2>&1; then \
		echo "📦 Installing committed (Conventional Commits linter)..."; \
		if command -v cargo-binstall >/dev/null 2>&1; then \
			cargo binstall -y committed; \
		else \
			cargo install committed; \
		fi; \
	fi
	@echo "🦀 Checking Rust components..."
	@if ! rustup component list --installed | grep -q rustfmt; then rustup component add rustfmt; fi
	@if ! rustup component list --installed | grep -q clippy; then rustup component add clippy; fi
	@echo "🔧 Git hooks configuration..."
	@if command -v pre-commit >/dev/null 2>&1; then \
		echo "💡 Pre-commit is available. Choose your hook system:"; \
		echo "   • Manual hooks (current): Already configured in .githooks/"; \
		echo "   • Enterprise pre-commit: Run 'make pre-commit-install'"; \
		echo "✅ Manual git hooks remain active"; \
	else \
		echo "✅ Manual git hooks configured in .githooks/"; \
		echo "💡 For enterprise-grade hooks: brew install pre-commit && make pre-commit-install"; \
	fi
	@echo "✅ Development environment setup complete"

dev-setup: setup ## Alias for setup
	@echo "Development setup complete. Run 'make help' for available commands."

# Cleaning
clean: ## Clean build artifacts and temporary files
	@echo "🧹 Cleaning build artifacts..."
	@cargo clean
	@rm -f debug_*
	@rm -f test_*
	@rm -f lcov.info cobertura.xml
	@rm -rf target/tarpaulin target/llvm-cov
	@find . -name "*.tmp" -delete
	@echo "✅ Clean complete"

# Testing
test: ## Run all tests
	@echo "🧪 Running tests..."
	@timeout 30s cargo test --lib --verbose

test-release: ## Run tests in release mode
	@echo "🧪 Running tests in release mode..."
	@timeout 30s cargo test --lib --release --verbose

test-all-features: ## Run tests with all features
	@echo "🧪 Running tests with all features..."
	@timeout 30s cargo test --lib --all-features --verbose || [ $$? -eq 124 ]

test-no-default: ## Run tests without default features
	@echo "🧪 Running tests without default features..."
	@timeout 30s cargo test --lib --no-default-features --verbose || [ $$? -eq 124 ]

test-nocapture: ## Run tests with output capture disabled
	@echo "🧪 Running tests with output visible..."
	@timeout 30s cargo test -- --nocapture

test-lib: ## Run library tests only
	@echo "🧪 Running library tests..."
	@timeout 30s cargo test --lib

test-integration: ## Run integration tests
	@echo "🧪 Running integration tests..."
	# `--tests` auto-discovers every `tests/*.rs` binary, so newly added
	# test files (e.g. `tests/directives.rs`) get picked up here without
	# being enumerated. Matches what CI / nextest sees end-to-end.
	@timeout 300s cargo test --tests

test-security: ## Run security-specific tests
	@echo "🔒 Running security tests..."
	@timeout 60s cargo test --test security_limits
	@timeout 60s cargo test --test security_limits test_nested_alias_expansion_limit

test-yaml-suite: ## Run upstream yaml-test-suite conformance harness
	@echo "🧪 Running yaml/yaml-test-suite conformance harness..."
	@git submodule update --init --recursive yaml-test-suite/data
	@timeout 300s cargo test -p yaml-test-suite -- --nocapture

# Code Quality
format: ## Format code
	@echo "🎨 Formatting code..."
	@cargo fmt

format-check: ## Check code formatting
	@echo "🎨 Checking code formatting..."
	@cargo fmt --all -- --check

lint: ## Run clippy lints
	@echo "📎 Running clippy..."
	@timeout 30s cargo clippy --all-targets --all-features -- -D warnings

lint-fix: ## Run clippy with automatic fixes
	@echo "📎 Running clippy with fixes..."
	@timeout 30s cargo clippy --all-targets --all-features --fix -- -D warnings

clippy-strict: ## Run clippy with strict CI settings
	@echo "📎 Running clippy with strict CI settings..."
	@timeout 180s cargo clippy --all-targets --all-features -- -D warnings -D clippy::all -D clippy::pedantic -W clippy::nursery \
		-A clippy::needless_raw_string_hashes \
		-A clippy::format_push_string \
		-A clippy::single_char_pattern \
		-A clippy::unreadable_literal \
		-A clippy::manual_string_new \
		-A clippy::write_with_newline \
		-A clippy::uninlined_format_args \
		-A clippy::semicolon_if_nothing_returned \
		-A clippy::explicit_iter_loop \
		-A clippy::inefficient_to_string \
		-A clippy::match_same_arms \
		-A clippy::doc_markdown \
		-A clippy::too_many_lines

# Security and Audit
audit: ## Run security audit
	@echo "🔒 Running security audit..."
	@if ! command -v cargo-audit >/dev/null; then cargo install cargo-audit; fi
	@cargo audit

deny: ## Run cargo deny checks
	@echo "🔒 Running cargo deny checks..."
	@if ! command -v cargo-deny >/dev/null; then cargo install cargo-deny; fi
	@timeout 15s cargo deny check

# Documentation
doc: ## Build documentation
	@echo "📚 Building documentation..."
	@cargo doc --all-features --no-deps

doc-open: ## Build and open documentation
	@echo "📚 Building and opening documentation..."
	@cargo doc --all-features --no-deps --open

doc-private: ## Build documentation including private items
	@echo "📚 Building documentation with private items..."
	@cargo doc --all-features --no-deps --document-private-items

# Benchmarking
bench: ## Run benchmarks
	@echo "⚡ Running benchmarks..."
	@cargo bench

bench-compile: ## Compile benchmarks only
	@echo "⚡ Compiling benchmarks..."
	@cargo bench --no-run

# Coverage
coverage: ## Generate test coverage report (same as CI)
	@echo "📊 Generating coverage report (CI-compatible)..."
	@if ! command -v cargo-tarpaulin >/dev/null; then \
		echo "Installing cargo-tarpaulin..."; \
		cargo install cargo-tarpaulin; \
	fi
	@cargo tarpaulin --lib --tests --out lcov --output-dir . --verbose --workspace --timeout 120 --exclude-files examples/* benches/*
	@echo "✅ Coverage report generated: lcov.info"
	@echo "📈 Coverage summary:"
	@cargo tarpaulin --lib --tests --print-summary --workspace --timeout 120 --exclude-files examples/* benches/*

coverage-html: ## Generate HTML coverage report
	@echo "📊 Generating HTML coverage report..."
	@if ! command -v cargo-tarpaulin >/dev/null; then \
		echo "Installing cargo-tarpaulin..."; \
		cargo install cargo-tarpaulin; \
	fi
	@cargo tarpaulin --lib --tests --out html --output-dir target/tarpaulin --verbose --workspace --timeout 120 --exclude-files examples/* benches/*
	@echo "✅ HTML coverage report generated in target/tarpaulin/"
	@echo "📂 Open target/tarpaulin/tarpaulin-report.html in your browser"

coverage-llvm: ## Generate coverage using llvm-cov (alternative)
	@echo "📊 Generating coverage with llvm-cov..."
	@if ! command -v cargo-llvm-cov >/dev/null; then \
		echo "Installing cargo-llvm-cov..."; \
		cargo install cargo-llvm-cov; \
	fi
	@cargo llvm-cov --lib --lcov --output-path lcov.info
	@echo "✅ Coverage report generated: lcov.info"

coverage-llvm-html: ## Generate HTML coverage with llvm-cov
	@echo "📊 Generating HTML coverage with llvm-cov..."
	@if ! command -v cargo-llvm-cov >/dev/null; then \
		echo "Installing cargo-llvm-cov..."; \
		cargo install cargo-llvm-cov; \
	fi
	@cargo llvm-cov --lib --html
	@echo "✅ HTML coverage report generated in target/llvm-cov/html/"

coverage-clean: ## Clean coverage artifacts
	@echo "🧹 Cleaning coverage artifacts..."
	@rm -f lcov.info cobertura.xml
	@rm -rf target/tarpaulin target/llvm-cov
	@echo "✅ Coverage artifacts cleaned"

coverage-view: coverage-html ## Generate and open HTML coverage report
	@echo "🌐 Opening coverage report in browser..."
	@if [ -f target/tarpaulin/tarpaulin-report.html ]; then \
		open target/tarpaulin/tarpaulin-report.html 2>/dev/null || xdg-open target/tarpaulin/tarpaulin-report.html 2>/dev/null || echo "Please open target/tarpaulin/tarpaulin-report.html manually"; \
	fi

coverage-install-tools: ## Install coverage tools
	@echo "📦 Installing coverage tools..."
	@cargo install cargo-tarpaulin cargo-llvm-cov
	@echo "✅ Coverage tools installed"

# Build
build: ## Build the project
	@echo "🔨 Building project..."
	@cargo build

build-release: ## Build in release mode
	@echo "🔨 Building project in release mode..."
	@timeout 30s cargo build --release

build-all-features: ## Build with all features
	@echo "🔨 Building project with all features..."
	@cargo build --all-features

examples: ## Run examples
	@echo "💡 Running examples..."
	@cargo run --example library_comparison

# Package and Release
package: ## Package the crate
	@echo "📦 Packaging crate..."
	@cargo package

package-list: ## List files that would be packaged
	@echo "📦 Listing package contents..."
	@cargo package --list

# Quality Checks (CI-like)
check: ## Run basic checks (build, test, format, lint)
	@echo "✅ Running basic checks..."
	@make format-check
	@make lint
	@make test
	@echo "✅ All basic checks passed"

check-all: ## Run all checks including audit and coverage
	@echo "✅ Running comprehensive checks..."
	@make format-check
	@make lint
	@make test-all-features
	@make test-no-default
	@make audit
	@make doc
	@make bench-compile
	@make check-markdown
	@echo "✅ All comprehensive checks passed"

ci: ## Run CI pipeline locally (same as GitHub Actions)
	@echo "🔄 Running CI pipeline locally..."
	@make format-check
	@make clippy-strict
	@make test-lib
	@make test-integration
	@make test-security
	@make deny
	@echo "✅ CI pipeline completed successfully!"

quick-check: ## Quick development checks (format, clippy, test)
	@echo "⚡ Running quick checks..."
	@make format
	@make lint
	@make test-lib
	@echo "✅ Quick checks passed!"

# Release Preparation
release-check: ## Check if ready for release
	@echo "🚀 Checking release readiness..."
	@make check-all
	@echo "Checking version consistency..."
	@CARGO_VERSION=$$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/'); \
	if [ -f GitVersion.yml ]; then \
		GITVERSION_VERSION=$$(grep 'next-version:' GitVersion.yml | sed 's/next-version: "\?\(.*\)"\?/\1/' | sed 's/-.*//'); \
		if [ "$$CARGO_VERSION" != "$$GITVERSION_VERSION" ]; then \
			echo "❌ Version mismatch: Cargo.toml=$$CARGO_VERSION, GitVersion.yml=$$GITVERSION_VERSION"; \
			exit 1; \
		fi; \
	fi
	@echo "✅ Release check passed"

# Release Management
release: ## Interactive release (manual process)
	@echo "🚀 Manual Release Process"
	@echo ""
	@echo "📋 Release Steps:"
	@echo "  1. Update version in Cargo.toml"
	@echo "  2. Update version in GitVersion.yml"
	@echo "  3. Run: make release-check"
	@echo "  4. Commit changes with: git commit -m 'chore: release version X.Y.Z [skip ci]'"
	@echo "  5. Create tag: git tag vX.Y.Z"
	@echo "  6. Push: git push origin main && git push origin vX.Y.Z"
	@echo ""
	@echo "💡 Or use GitHub Actions 'Manual Version Bump' workflow"

release-patch: ## Guide for patch release
	@echo "🚀 Patch Release Guide"
	@$(MAKE) release

release-minor: ## Guide for minor release
	@echo "🚀 Minor Release Guide"
	@$(MAKE) release

release-major: ## Guide for major release
	@echo "🚀 Major Release Guide"
	@$(MAKE) release

# Development Workflow
pre-commit: ## Run pre-commit checks
	@echo "🔍 Running pre-commit checks..."
	@make format
	@make lint
	@make test
	@echo "✅ Pre-commit checks passed"

pre-push: ## Run pre-push checks
	@echo "🔍 Running pre-push checks..."
	@make check-all
	@echo "✅ Pre-push checks passed"

# Commit Message Validation
commit-lint: ## Test commit message format (via committed)
	@echo "📝 Testing commit message format..."
	@if [ -z "$(MSG)" ]; then \
		echo "Usage: make commit-lint MSG='feat: add new feature'"; \
		exit 1; \
	fi
	@if ! command -v committed >/dev/null 2>&1; then \
		echo "❌ committed not found. Install with: cargo install committed"; \
		exit 1; \
	fi
	@printf '%s\n' "$(MSG)" | committed --commit-file -
	@echo "✅ Commit message format is valid"

# Git Workflow Helpers
git-status: ## Show git status with helpful formatting
	@git status --porcelain | if [ $$(wc -l) -eq 0 ]; then \
		echo "✅ Working directory clean"; \
	else \
		echo "📂 Working directory changes:"; \
		git status --short; \
	fi

# Pre-commit Hooks Management
pre-commit-install: ## Install pre-commit hooks (replaces manual git hooks)
	@echo "🔧 Installing pre-commit hooks..."
	@if ! command -v pre-commit >/dev/null; then \
		echo "❌ pre-commit not found. Install with: brew install pre-commit"; \
		exit 1; \
	fi
	@echo "📝 Switching from manual git hooks to pre-commit..."
	@git config --unset-all core.hooksPath || true
	@pre-commit install --install-hooks
	@pre-commit install --hook-type commit-msg
	@echo "✅ Pre-commit hooks installed (manual .githooks disabled)"

pre-commit-run: ## Run pre-commit hooks on all files
	@echo "🔍 Running pre-commit hooks on all files..."
	@timeout 120s pre-commit run --all-files

pre-commit-dev: ## Run pre-commit hooks (without timeout for development)
	@echo "🔍 Running pre-commit hooks..."
	@pre-commit run --all-files

pre-commit-update: ## Update pre-commit hooks to latest versions
	@echo "⬆️  Updating pre-commit hooks..."
	@pre-commit autoupdate

pre-commit-clean: ## Clean pre-commit cache
	@echo "🧹 Cleaning pre-commit cache..."
	@pre-commit clean

pre-commit-rust: ## Run only Rust-specific pre-commit hooks
	@echo "🦀 Running Rust-specific pre-commit hooks..."
	@pre-commit run rust-fmt rust-clippy rust-check rust-test rust-doc

pre-commit-quick: ## Run quick pre-commit hooks (skip tests and audits)
	@echo "⚡ Running quick pre-commit hooks..."
	@SKIP=rust-test,rust-coverage,rust-audit,rust-deny pre-commit run --all-files

pre-commit-security: ## Run security-focused pre-commit hooks
	@echo "🔒 Running security-focused pre-commit hooks..."
	@pre-commit run detect-private-key detect-secrets rust-audit rust-deny

# Installation and Dependencies
install-tools: ## Install additional development tools
	@echo "🔧 Installing development tools..."
	@if ! command -v cargo-audit >/dev/null; then cargo install cargo-audit; fi
	@if ! command -v cargo-llvm-cov >/dev/null; then cargo install cargo-llvm-cov; fi
	@if ! command -v cargo-criterion >/dev/null; then cargo install cargo-criterion; fi
	@if ! command -v cargo-flamegraph >/dev/null; then cargo install flamegraph; fi
	@echo "✅ Development tools installed"

# Version Information
version: ## Show version information
	@echo "📋 Version Information:"
	@echo "Cargo version: $$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')"
	@if [ -f GitVersion.yml ]; then \
		echo "GitVersion next: $$(grep 'next-version:' GitVersion.yml | sed 's/next-version: "\?\(.*\)"\?/\1/')"; \
	fi
	@if command -v git >/dev/null && git rev-parse --git-dir >/dev/null 2>&1; then \
		echo "Git commit: $$(git rev-parse --short HEAD)"; \
		echo "Git branch: $$(git branch --show-current)"; \
	fi

# Debug and Troubleshooting
debug-env: ## Show development environment information
	@echo "🔍 Development Environment:"
	@echo "Rust version: $$(rustc --version)"
	@echo "Cargo version: $$(cargo --version)"
	@if command -v committed >/dev/null; then echo "committed version: $$(committed --version)"; fi
	@echo "Git hooks path: $$(git config --get core.hooksPath || echo 'default')"
	@echo "Commit template: $$(git config --get commit.template || echo 'none')"

# Markdown and Documentation
check-markdown: ## Check markdown formatting
	@echo "📝 Checking markdown formatting..."
	@for file in *.md; do \
		if [ -f "$$file" ]; then \
			echo "Checking $$file..."; \
			echo "=== MD022 Issues in $$file ==="; \
			awk 'NR > 1 && /^#{1,6} / && prev_line != "" {print "Line " NR ": Missing blank line before heading: " $$0} {prev_line = $$0}' "$$file" || true; \
			echo "=== MD032 Issues in $$file ==="; \
			awk 'NR > 1 && (/^[-*+] / || /^[0-9]+\. /) && prev_line != "" && prev_line !~ /^[-*+] / && prev_line !~ /^[0-9]+\. / {print "Line " NR ": Missing blank line before list: " $$0} {prev_line = $$0}' "$$file" || true; \
			echo ""; \
		fi; \
	done

fix-markdown: ## Fix common markdown formatting issues
	@echo "📝 Fixing markdown issues (MD022: blanks around headings, MD031: blanks around fences, MD032: blanks around lists, bold-as-headings)..."
	@for file in *.md docs/*.md; do \
		if [ -f "$$file" ]; then \
			echo "Processing $$file..."; \
			awk 'BEGIN { \
				prev_line = ""; \
				in_code_block = 0; \
				last_was_blank = 0; \
				need_blank_after = 0 \
			} \
			/^```/ { \
				if (!in_code_block) { \
					if (NR > 1 && !last_was_blank) { \
						print "" \
					} \
					in_code_block = 1; \
					print; \
					need_blank_after = 0 \
				} else { \
					in_code_block = 0; \
					print; \
					need_blank_after = 1 \
				} \
				prev_line = $$0; \
				last_was_blank = 0; \
				next \
			} \
			in_code_block { \
				print; \
				prev_line = $$0; \
				last_was_blank = 0; \
				next \
			} \
			/^#{1,6} \*\*.*\*\*/ { \
				gsub(/\*\*/, "", $$0) \
			} \
			/^#{1,6} / { \
				if (need_blank_after) { \
					print ""; \
					need_blank_after = 0 \
				} \
				if (NR > 1 && !last_was_blank && prev_line !~ /^#{1,6} /) { \
					print "" \
				} \
				print; \
				prev_line = $$0; \
				last_was_blank = 0; \
				next \
			} \
			/^[-*+] / || /^[0-9]+\. / { \
				if (need_blank_after) { \
					print ""; \
					need_blank_after = 0 \
				} \
				if (NR > 1 && !last_was_blank && prev_line !~ /^[-*+] / && prev_line !~ /^[0-9]+\. / && prev_line !~ /^  /) { \
					print "" \
				} \
				print; \
				prev_line = $$0; \
				last_was_blank = 0; \
				next \
			} \
			/^$$/ { \
				if (need_blank_after) { \
					need_blank_after = 0 \
				} \
				print; \
				prev_line = $$0; \
				last_was_blank = 1; \
				next \
			} \
			{ \
				if (need_blank_after) { \
					print ""; \
					need_blank_after = 0 \
				} \
				if (prev_line ~ /^#{1,6} / && !last_was_blank) { \
					print "" \
				} \
				if ((prev_line ~ /^[-*+] / || prev_line ~ /^[0-9]+\. /) && $$0 !~ /^[-*+] / && $$0 !~ /^[0-9]+\. / && $$0 !~ /^  / && !last_was_blank) { \
					print "" \
				} \
				print; \
				prev_line = $$0; \
				last_was_blank = 0 \
			}' "$$file" > "$$file.tmp" && mv "$$file.tmp" "$$file"; \
			echo "Fixed $$file"; \
		fi; \
	done
	@echo "✅ Markdown formatting completed!"

check-markdown-detailed: ## Check markdown with detailed line numbers
	@echo "📝 Detailed markdown check..."
	@for file in *.md; do \
		if [ -f "$$file" ]; then \
			echo "=== Analyzing $$file ==="; \
			awk ' \
			BEGIN { prev_line = ""; line_num = 0; issues = 0 } \
			{ line_num++ } \
			/^#{1,6} / { \
				if (line_num > 1 && prev_line !~ /^$$/) { \
					print "MD022 - Line " line_num ": Missing blank line before heading: " $$0; \
					issues++ \
				} \
			} \
			/^[-*+] / || /^[0-9]+\. / { \
				if (line_num > 1 && prev_line !~ /^$$/ && prev_line !~ /^[-*+] / && prev_line !~ /^[0-9]+\. / && prev_line !~ /^  /) { \
					print "MD032 - Line " line_num ": Missing blank line before list: " $$0; \
					issues++ \
				} \
			} \
			{ prev_line = $$0 } \
			END { \
				if (issues == 0) print "✅ No issues found"; \
				else print "❌ Found " issues " issues" \
			}' "$$file"; \
			echo ""; \
		fi; \
	done

# Aliases for common tasks
fmt: format ## Alias for format
build-all: build-all-features ## Alias for build-all-features
test-verbose: test-nocapture ## Alias for test-nocapture
docs: doc ## Alias for doc
checks: check ## Alias for check
