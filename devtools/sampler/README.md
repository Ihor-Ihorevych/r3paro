# sampler

A sampling profiler for the Windows builds that needs no administrator rights (samply and WPR do).

    # a frame-pointer build in its own target directory, so the normal one is left alone
    CARGO_TARGET_DIR=target-prof RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release -p hp3-run
    cd devtools/sampler && cargo build --release
    SAMPLER_DELAY_MS=3000 devtools/sampler/target/release/sampler.exe target-prof/release/hp3-run.exe target-prof/release/hp3_run.pdb level ../spike-hp3 HP3_InsideHub 3000

It prints the functions where the program was (self) and the functions on the stack (inclusive), as a share of the samples. Inlined
functions are reported as their own entries. Env: `SAMPLER_DELAY_MS` (skip the load), `SAMPLER_MAXMS` (stop and kill the program),
`SAMPLER_LEAF=a,b` (callers of functions whose name contains a or b), `SAMPLER_ROOT=name` (only samples with that function on the stack).
Only the first frame is reliable: the stack walk follows frame pointers and breaks in places, so read "self", not "inclusive".
