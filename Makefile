.PHONY: install build clean dev

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
