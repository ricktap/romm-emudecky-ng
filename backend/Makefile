# Builds the native backend into out/ (the Decky CLI ships out/ as bin/).

BIN := romm-emudecky-ng-backend

.PHONY: all clean

all:
	cargo build --release --locked
	mkdir -p out
	cp target/release/$(BIN) out/$(BIN)
	chmod 755 out/$(BIN)

clean:
	cargo clean
	rm -rf out
