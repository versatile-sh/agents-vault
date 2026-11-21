.PHONY: check test semgrep

check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets
	$(MAKE) --no-print-directory test
	$(MAKE) --no-print-directory semgrep

test:
	cargo test --workspace

SEMGREP_RULES := https://raw.githubusercontent.com/versatile-sh/semgrep-rules/v0.1.2/rules/versatile.yml

semgrep:
	semgrep scan --metrics=off --disable-version-check --error --severity WARNING --severity ERROR \
		--config p/default --config p/secrets --config p/javascript --config p/rust --config p/python --config $(SEMGREP_RULES) \
		--baseline-commit "$$(git merge-base HEAD origin/main)"
