//! v0.3.0 throughput benchmarks: AES-CTR re-encryption path (the per-byte
//! hot loop of the bridge), `MsgSplitter`, and per-connection `CryptoCtx`
//! setup cost. Run with `cargo bench -p tg-ws-proxy-core`.

use aes::cipher::StreamCipher;
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use tg_ws_proxy_core::config::{PROTO_ABRIDGED_INT, PROTO_TAG_ABRIDGED};
use tg_ws_proxy_core::crypto::CryptoCtx;
use tg_ws_proxy_core::handshake::generate_relay_init;
use tg_ws_proxy_core::splitter::MsgSplitter;

fn bench_ctx() -> CryptoCtx {
    let prekey_iv = [0xABu8; 48];
    let secret = [0xCDu8; 16];
    let relay_init = generate_relay_init(&PROTO_TAG_ABRIDGED, 2);
    CryptoCtx::new(&prekey_iv, &secret, &relay_init)
}

/// Mirror of the bridge upload path: decrypt with client key, re-encrypt
/// with Telegram key, fully in place.
fn reencrypt(c: &mut Criterion) {
    let mut group = c.benchmark_group("bridge_reencrypt");
    for size in [4096usize, 65536, 262144] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_function(format!("{size}B"), |b| {
            b.iter_batched(
                || vec![0x55u8; size],
                |mut buf| {
                    let mut ctx = bench_ctx();
                    ctx.clt_dec.apply_keystream(&mut buf);
                    ctx.tg_enc.apply_keystream(&mut buf);
                    black_box(buf);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn splitter(c: &mut Criterion) {
    let mut group = c.benchmark_group("splitter");
    for size in [1500usize, 65536] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_function(format!("{size}B"), |b| {
            b.iter_batched(
                || {
                    let relay_init = generate_relay_init(&PROTO_TAG_ABRIDGED, 2);
                    (MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT), vec![0x77u8; size])
                },
                |(mut sp, chunk)| {
                    let parts = sp.split(&chunk);
                    black_box(parts);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn ctx_setup(c: &mut Criterion) {
    c.bench_function("crypto_ctx_new", |b| {
        b.iter_batched(
            || {
                (
                    [0xABu8; 48],
                    [0xCDu8; 16],
                    generate_relay_init(&PROTO_TAG_ABRIDGED, 2),
                )
            },
            |(prekey_iv, secret, relay_init)| {
                black_box(CryptoCtx::new(&prekey_iv, &secret, &relay_init));
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, reencrypt, splitter, ctx_setup);
criterion_main!(benches);
