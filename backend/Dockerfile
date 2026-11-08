# Built by the Decky CLI (`decky plugin build`) whenever backend/ exists. The CLI
# mounts backend/ at /backend and the plugin's bin/ staging dir at /backend/out,
# then runs this image; whatever the Makefile leaves in out/ ships as bin/.
#
# holo-toolchain-rust is SteamOS (holo) with rustup + gcc, so the binary links
# against the SteamOS glibc baseline and also runs on Bazzite (newer glibc).
FROM ghcr.io/steamdeckhomebrew/holo-toolchain-rust:latest

# The image's "stable" is whatever was current when it was built; install the
# pinned toolchain from rust-toolchain.toml so the build is reproducible.
RUN rustup toolchain install 1.99.1 --profile minimal \
 && chmod -R a+rwX /.rustup /.cargo

# entrypoint.sh should always be located in the backend folder
ENTRYPOINT [ "/backend/entrypoint.sh" ]
