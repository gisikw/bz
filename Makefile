.PHONY: install build clean dev reset

# Build and copy to ./bin for local use
install: build
	@mkdir -p bin
	@cp target/release/bz bin/
	@cp target/release/bzd bin/
	@cp target/release/bzc bin/
	@echo "Installed to ./bin/"
	@echo "Symlink with: ln -sf $(PWD)/bin/bz ~/.local/bin/ && ln -sf $(PWD)/bin/bzd ~/.local/bin/ && ln -sf $(PWD)/bin/bzc ~/.local/bin/"

# Development build (faster, unoptimized)
dev:
	cargo build
	@mkdir -p bin
	@cp target/debug/bz bin/
	@cp target/debug/bzd bin/
	@cp target/debug/bzc bin/
	@echo "Dev build installed to ./bin/"

build:
	cargo build --release

clean:
	cargo clean
	rm -rf bin/

# Reset all bz state (for clean debugging)
# Kills processes, removes session state, and clears Matrix database
reset:
	@echo "Stopping bz processes..."
	-@pkill -f "bzd" 2>/dev/null || true
	-@pkill -f "bzc" 2>/dev/null || true
	-@pkill -f "conduit" 2>/dev/null || true
	@sleep 1
	@echo "Removing session state..."
	-@rm -f ~/.local/state/bz/sessions/*.sock 2>/dev/null || true
	-@rm -f ~/.local/state/bz/sessions/*.pid 2>/dev/null || true
	@echo "Removing Conduit config and database..."
	-@rm -f ~/.local/share/bz/conduit.toml 2>/dev/null || true
	-@rm -rf ~/.local/share/bz/matrix 2>/dev/null || true
	@mkdir -p ~/.local/share/bz/matrix
	@echo "Clearing bz log..."
	-@rm -f ~/.local/share/bz/bz.log 2>/dev/null || true
	@echo "Reset complete. Run: make dev && ./bin/bz --config=./bz.test.toml"
