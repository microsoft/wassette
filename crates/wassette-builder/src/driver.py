# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.
#
# Fixed trusted guest driver. This file is never executed on the host.
import base64
import json
import os
import selectors
import subprocess
import hyperlight

with open("/input/driver.json") as f:
    config = json.load(f)

os.environ.clear()
os.environ.update({
    "LD_LIBRARY_PATH": "/opt/rust/lib",
    "PATH": "/opt/rust/bin",
    "TMPDIR": "/tmp",
    "LANG": "C.UTF-8",
    "HOME": "/tmp",
})
os.makedirs("/tmp/wassette-build", exist_ok=True)
os.chdir("/tmp/wassette-build")
root = "/opt/rust"
libs = root + "/lib/rustlib/wasm32-wasip2/lib/self-contained"
arch = config["compiler_arch"]
if arch not in ("aarch64", "x86_64"):
    raise RuntimeError("unsupported compiler profile architecture")
diagnostics_used = 0

def emit(kind, data):
    for start in range(0, len(data), 16384):
        hyperlight.call("builder-output", kind,
                        base64.b64encode(data[start:start + 16384]).decode("ascii"))

def fail(kind):
    emit("failure", json.dumps(kind).encode("ascii"))

def run(argv, label, channel):
    global diagnostics_used
    # All buffers live in bounded guest scratch, never on the host filesystem.
    # Keep the python-shell image's verified vfork/pipe spawn path.
    remaining = 256 if channel == "profile" else config["diagnostics_bytes"] - diagnostics_used
    chunks = []
    size = 0
    with subprocess.Popen(argv, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE) as process:
        with selectors.PollSelector() as poll:
            for pipe in (process.stdout, process.stderr):
                poll.register(pipe, selectors.EVENT_READ)
            while poll.get_map():
                for key, _ in poll.select():
                    chunk = os.read(key.fd, min(8192, remaining - size + 1))
                    if not chunk:
                        poll.unregister(key.fileobj)
                        key.fileobj.close()
                        continue
                    size += len(chunk)
                    if size > remaining:
                        fail("profile" if channel == "profile" else "diagnostics")
                        process.kill()
                        raise RuntimeError(label + " diagnostics exceed budget")
                    chunks.append(chunk)
        status = process.wait()
    diagnostics = b"".join(chunks)
    if diagnostics and channel != "profile":
        diagnostics_used += len(diagnostics)
        emit(channel + "-diagnostics", diagnostics)
    if status != 0:
        fail(channel)
        raise RuntimeError(label + " failed with status " + str(status))
    return diagnostics

version = run([root + "/bin/rustc", "--version"], "compiler profile", "profile")
if not version.startswith(b"rustc 1.98.1 "):
    fail("profile")
    raise RuntimeError("unsupported compiler profile: expected rustc 1.98.1")

run([root + "/bin/rustc", "--edition=2024", "--target=wasm32-wasip2",
     "--error-format=short", "--color=never",
     "--crate-name=generated_component", "--crate-type=staticlib",
     "--cfg", 'feature="std"', "--cfg", 'feature="async"',
     "-C", "panic=abort", "-C", "opt-level=s", "-C", "codegen-units=1",
     "--remap-path-prefix=/input=builder-input",
     "-o", "component.a", "/input/component.rs"], "Rust compilation", "rust")

exports = ["--export=" + name for name in config["exports"]]
run([root + "/lib/rustlib/" + arch + "-unknown-linux-gnu/bin/wasm-component-ld",
     "--wasm-ld-path=" + root + "/bin/wasm-ld", "--no-entry",
     libs + "/crt1-reactor.o", "component.a", "-L", libs, "-lc",
     "--gc-sections", "-O2", "--strip-all", "--threads=1",
     "-o", "component.wasm"] + exports, "component linking", "link")

size = os.stat("component.wasm").st_size
if size == 0 or size > config["wasm_bytes"]:
    fail("output")
    raise RuntimeError("Wasm output exceeds budget or is empty")
with open("component.wasm", "rb") as output:
    total = 0
    while True:
        chunk = output.read(16384)
        if not chunk:
            break
        total += len(chunk)
        if total > config["wasm_bytes"]:
            fail("output")
            raise RuntimeError("Wasm output grew beyond budget")
        emit("wasm", chunk)
