.PHONY: install build clean

# Build and copy to ./bin for local use
install: build
	@mkdir -p bin
	@cp target/release/bz bin/
	@cp target/release/bzd bin/
	@echo "Installed to ./bin/bz and ./bin/bzd"
	@echo "Symlink with: ln -sf $(PWD)/bin/bz ~/.local/bin/ && ln -sf $(PWD)/bin/bzd ~/.local/bin/"

# Development build (faster, unoptimized)
dev:
	cargo build
	@mkdir -p bin
	@cp target/debug/bz bin/
	@cp target/debug/bzd bin/
	@echo "Dev build installed to ./bin/"

build:
	cargo build --release

clean:
	cargo clean
	rm -rf bin/
