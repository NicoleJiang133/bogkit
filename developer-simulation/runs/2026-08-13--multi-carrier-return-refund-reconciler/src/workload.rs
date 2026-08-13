use crate::{AuthorizedLine, Event, PaymentStatus, ReturnAuthorization, Snapshot};

const RETURN_COUNT: usize = 25_000;
const ADVERSARIAL_RETURN_COUNT: usize = 500;
const EVENT_COUNT: usize = 250_000;

/// Generate the exact synthetic representative shape described by the brief.
///
/// The seed controls only deterministic ingestion identifiers and filler
/// timestamps; generated data contains identifiers and integer amounts only.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn generate_representative(seed: u64) -> Snapshot {
    let mut returns = Vec::with_capacity(RETURN_COUNT);
    let mut events = Vec::with_capacity(EVENT_COUNT);
    let mut ingestion_id = seed.max(1);

    for return_index in 0..RETURN_COUNT {
        let return_id = if return_index < ADVERSARIAL_RETURN_COUNT {
            format!("adv-return-{return_index:05}")
        } else {
            format!("return-{return_index:05}")
        };
        let line_count = if return_index < 20_000 { 3 } else { 2 };
        let parcel_count = if return_index < 15_000 { 2 } else { 1 };
        let mut lines = Vec::with_capacity(line_count);
        let mut captured_cents = 0_u64;
        let mut first_scan = None;

        for line_index in 0..line_count {
            let remainder_case =
                return_index < ADVERSARIAL_RETURN_COUNT && return_index % 5 == 2 && line_index == 0;
            let paid_subtotal_cents = if remainder_case {
                901
            } else {
                900 + ((return_index + line_index) % 97) as u64
            };
            let tax_cents = 80;
            let discount_cents = 30;
            captured_cents += paid_subtotal_cents + tax_cents - discount_cents;
            let line_id = format!("line-{return_index:05}-{line_index:02}");
            let sku = format!("sku-{line_index:02}");
            lines.push(AuthorizedLine {
                line_id: line_id.clone(),
                sku: sku.clone(),
                authorized_qty: if remainder_case { 2 } else { 1 },
                paid_subtotal_cents,
                tax_cents,
                discount_cents,
                prior_successful_refund_cents: 0,
            });
            let scan = Event::WarehouseScan {
                event_id: format!("scan-{return_index:05}-{line_index:02}"),
                source_system_id: format!("warehouse-{}", return_index % 2),
                source_timestamp: 1_000 + line_index as u64,
                ingestion_id,
                return_id: return_id.clone(),
                parcel_id: format!("parcel-{return_index:05}-{:02}", line_index % parcel_count),
                sku,
                quantity: if remainder_case { 2 } else { 1 },
            };
            ingestion_id += 1;
            if line_index == 0 {
                first_scan = Some(scan.clone());
            }
            events.push(scan);
        }

        for parcel_index in 0..parcel_count {
            events.push(Event::Carrier {
                event_id: format!("carrier-{return_index:05}-{parcel_index:02}"),
                source_system_id: format!("carrier-{}", return_index % 3),
                source_timestamp: 2_000_u64.saturating_sub(parcel_index as u64),
                ingestion_id,
                return_id: return_id.clone(),
                parcel_id: format!("parcel-{return_index:05}-{parcel_index:02}"),
                status: "delivered".to_string(),
            });
            ingestion_id += 1;
        }

        let first_line_id = lines[0].line_id.clone();
        let payment = Event::PaymentResult {
            event_id: format!("payment-{return_index:05}-failed"),
            source_system_id: "payment-provider".to_string(),
            source_timestamp: 3_000,
            ingestion_id,
            return_id: return_id.clone(),
            line_id: first_line_id.clone(),
            refund_id: format!("refund-{return_index:05}"),
            amount_cents: lines[0].paid_subtotal_cents + lines[0].tax_cents
                - lines[0].discount_cents,
            status: PaymentStatus::Failed,
        };
        ingestion_id += 1;
        events.push(payment.clone());

        if return_index < ADVERSARIAL_RETURN_COUNT {
            let Some(base_scan) = first_scan else {
                continue;
            };
            match return_index % 5 {
                0 => events.push(base_scan.clone()),
                1 => {
                    let Event::WarehouseScan {
                        event_id,
                        source_system_id,
                        source_timestamp,
                        ingestion_id: original_ingestion_id,
                        return_id: scan_return_id,
                        parcel_id,
                        sku,
                        ..
                    } = base_scan
                    else {
                        unreachable!();
                    };
                    events.push(Event::WarehouseScan {
                        event_id,
                        source_system_id,
                        source_timestamp,
                        ingestion_id: original_ingestion_id,
                        return_id: scan_return_id,
                        parcel_id,
                        sku,
                        quantity: 2,
                    });
                }
                2 => events.push(Event::Correction {
                    event_id: format!("correction-{return_index:05}"),
                    source_system_id: "warehouse-0".to_string(),
                    source_timestamp: 4_000,
                    ingestion_id: 1,
                    return_id: return_id.clone(),
                    target_event_id: base_scan.event_id().to_string(),
                    replacement_sku: "sku-00".to_string(),
                    replacement_quantity: 1,
                }),
                3 => events.push(Event::WarehouseScan {
                    event_id: format!("scan-{return_index:05}-excess"),
                    source_system_id: "warehouse-1".to_string(),
                    source_timestamp: 4_000,
                    ingestion_id,
                    return_id: return_id.clone(),
                    parcel_id: format!("parcel-{return_index:05}-01"),
                    sku: "sku-00".to_string(),
                    quantity: 1,
                }),
                _ => events.push(Event::WarehouseScan {
                    event_id: format!("scan-{return_index:05}-unknown"),
                    source_system_id: "warehouse-0".to_string(),
                    source_timestamp: 4_000,
                    ingestion_id,
                    return_id: return_id.clone(),
                    parcel_id: format!("parcel-{return_index:05}-00"),
                    sku: "sku-unauthorized".to_string(),
                    quantity: 1,
                }),
            }
            ingestion_id += 1;

            match return_index % 5 {
                0 | 3 => events.push(payment_variant(
                    &return_id,
                    &first_line_id,
                    return_index,
                    "pending",
                    PaymentStatus::Pending,
                    ingestion_id,
                    lines[0].paid_subtotal_cents + lines[0].tax_cents - lines[0].discount_cents,
                )),
                1 | 4 => {
                    events.push(payment_variant(
                        &return_id,
                        &first_line_id,
                        return_index,
                        "successful",
                        PaymentStatus::Successful,
                        ingestion_id,
                        lines[0].paid_subtotal_cents + lines[0].tax_cents - lines[0].discount_cents,
                    ));
                    ingestion_id += 1;
                    events.push(payment_variant(
                        &return_id,
                        &first_line_id,
                        return_index,
                        "delayed-pending",
                        PaymentStatus::Pending,
                        ingestion_id,
                        lines[0].paid_subtotal_cents + lines[0].tax_cents - lines[0].discount_cents,
                    ));
                }
                _ => events.push(payment),
            }
            ingestion_id += 1;
        }

        returns.push(ReturnAuthorization {
            return_id,
            captured_cents,
            lines,
        });
    }

    let mut filler = 0_usize;
    while events.len() < EVENT_COUNT {
        let return_index = filler % RETURN_COUNT;
        let return_id = if return_index < ADVERSARIAL_RETURN_COUNT {
            format!("adv-return-{return_index:05}")
        } else {
            format!("return-{return_index:05}")
        };
        let parcel_index = if return_index < 15_000 { filler % 2 } else { 0 };
        events.push(Event::Carrier {
            event_id: format!("carrier-filler-{filler:06}"),
            source_system_id: format!("carrier-{}", filler % 3),
            source_timestamp: 10_000_u64.saturating_sub((filler % 1_000) as u64),
            ingestion_id,
            return_id,
            parcel_id: format!("parcel-{return_index:05}-{parcel_index:02}"),
            status: if filler.is_multiple_of(2) {
                "in_transit".to_string()
            } else {
                "delivered".to_string()
            },
        });
        ingestion_id += 1;
        filler += 1;
    }

    Snapshot {
        snapshot_id: format!("representative-{seed}"),
        returns,
        events,
    }
}

fn payment_variant(
    return_id: &str,
    line_id: &str,
    return_index: usize,
    suffix: &str,
    status: PaymentStatus,
    ingestion_id: u64,
    amount_cents: u64,
) -> Event {
    Event::PaymentResult {
        event_id: format!("payment-{return_index:05}-{suffix}"),
        source_system_id: "payment-provider".to_string(),
        source_timestamp: 3_001,
        ingestion_id,
        return_id: return_id.to_string(),
        line_id: line_id.to_string(),
        refund_id: format!("refund-{return_index:05}"),
        amount_cents,
        status,
    }
}

/// Shuffle returns, lines, and events with a small deterministic PRNG.
pub fn deterministic_shuffle(snapshot: &mut Snapshot, seed: u64) {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    shuffle_slice(&mut snapshot.events, &mut state);
    shuffle_slice(&mut snapshot.returns, &mut state);
    for authorization in &mut snapshot.returns {
        shuffle_slice(&mut authorization.lines, &mut state);
    }
}

fn shuffle_slice<T>(values: &mut [T], state: &mut u64) {
    for index in (1..values.len()).rev() {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        let upper = u64::try_from(index + 1).expect("slice length fits u64");
        let swap_index = usize::try_from(*state % upper).expect("index fits usize");
        values.swap(index, swap_index);
    }
}
