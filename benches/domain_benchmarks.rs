use criterion::{Criterion, criterion_group, criterion_main};
use solana_compliance_relayer::domain::{SubmitTransferRequest, TransferType};
use std::hint::black_box;
use validator::Validate;

fn bench_validation(c: &mut Criterion) {
    let request = SubmitTransferRequest {
        from_address: "AddressA".to_string(),
        to_address: "AddressB".to_string(),
        transfer_details: TransferType::Public {
            amount: 10_500_000_000,
        },
        token_mint: None,
    };

    c.bench_function("validate_transfer_request", |b| {
        b.iter(|| {
            let _ = black_box(&request).validate();
        })
    });
}

criterion_group!(benches, bench_validation);
criterion_main!(benches);
