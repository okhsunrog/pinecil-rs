use uom::si::{electric_current::milliampere, electric_potential::millivolt};
use usbpd::protocol_layer::message::data::{
    request::{EprRequestDataObject, FixedVariableSupply, PowerSource},
    source_capabilities::{PowerDataObject, SourceCapabilities},
};

pub const TARGET_MV: u32 = 20_000;
pub const EPR_TARGET_MV: u32 = 28_000;
pub const EPR_ENABLED: bool = !cfg!(feature = "pd-spr-only");
// SPR does not perform cable-current discovery. Successful EPR entry includes
// the source's EPR cable qualification; still never exceed the advertised PDO.
pub const SPR_MAX_CURRENT_MA: u32 = 3_000;
pub const EPR_MAX_CURRENT_MA: u32 = 5_000;
pub const EPR_OPERATIONAL_WATTS: u32 = EPR_TARGET_MV * EPR_MAX_CURRENT_MA / 1_000_000;
pub const SUPPLY_MARGIN_MA: u32 = 200;

/// Estimated current while the heater MOSFET is on, rounded upward. Reducing
/// the burst's duration reduces average power but does not reduce this current.
pub fn heater_on_current_ma(vin_mv: u32, resistance_mohm: u32) -> Option<u32> {
    if !(4000..=8000).contains(&resistance_mohm) || vin_mv > 30_000 {
        return None;
    }
    Some((u64::from(vin_mv) * 1000).div_ceil(u64::from(resistance_mohm)) as u32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerStatus {
    Waiting,
    Requesting { mv: u32 },
    Contract { mv: u32, ma: u32 },
    Error,
}

pub struct Choice {
    pub request: PowerSource,
    pub mv: u32,
    pub ma: u32,
}

/// Prefer the highest fixed voltage up to 20 V in SPR or 28 V in EPR. Once a
/// cartridge is identified, require enough current for its full on-window load
/// plus the board margin, allowing 5% source-voltage tolerance. Before probing,
/// an idle contract is allowed; the independent heater interlock requires R.
/// Never select PPS/AVS, an SPR >20 V PDO, or any voltage above 28 V.
pub fn choose(caps: &SourceCapabilities, resistance_mohm: Option<u32>) -> Option<Choice> {
    let safe = caps.vsafe_5v()?;
    if safe.voltage().get::<millivolt>() != 5_000 || safe.max_current().get::<milliampere>() == 0 {
        return None;
    }
    let epr = caps.is_epr_capabilities();
    let current_limit = if epr {
        EPR_MAX_CURRENT_MA
    } else {
        SPR_MAX_CURRENT_MA
    };
    let candidates = caps.spr_pdos().chain(caps.epr_pdos().filter(|_| epr));
    let (position, pdo) = candidates
        .filter_map(|(position, pdo)| {
            let PowerDataObject::FixedSupply(f) = pdo else {
                return None;
            };
            let mv = f.voltage().get::<millivolt>();
            let ma = f.max_current().get::<milliampere>().min(current_limit);
            let max_mv = if epr { EPR_TARGET_MV } else { TARGET_MV };
            if !(9000..=max_mv).contains(&mv) || ma <= SUPPLY_MARGIN_MA {
                return None;
            }
            if let Some(r) = resistance_mohm {
                let current = heater_on_current_ma(mv * 105 / 100, r)?;
                if current + SUPPLY_MARGIN_MA > ma {
                    return None;
                }
            }
            Some((position, *pdo))
        })
        .max_by_key(|(_, pdo)| match pdo {
            PowerDataObject::FixedSupply(f) => f.voltage().get::<millivolt>(),
            _ => 0,
        })
        .unwrap_or((1, PowerDataObject::FixedSupply(*safe)));
    let PowerDataObject::FixedSupply(selected) = pdo else {
        unreachable!()
    };
    let mv = selected.voltage().get::<millivolt>();
    let ma = selected
        .max_current()
        .get::<milliampere>()
        .min(current_limit);
    let request = FixedVariableSupply(0)
        .with_object_position(position)
        .with_raw_operating_current((ma / 10) as u16)
        .with_raw_max_operating_current((ma / 10) as u16)
        .with_no_usb_suspend(true)
        .with_usb_communications_capable(false)
        .with_epr_mode_capable(epr || (EPR_ENABLED && caps.epr_mode_capable()));
    Some(Choice {
        request: if epr {
            // EPR_Request always includes both the RDO and a copy of the PDO,
            // including a fallback that requests an SPR voltage while in EPR.
            PowerSource::EprRequest(EprRequestDataObject {
                rdo: request.0,
                pdo,
            })
        } else {
            PowerSource::FixedVariableSupply(request)
        },
        mv,
        ma,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use usbpd::protocol_layer::message::data::source_capabilities::FixedSupply;

    fn caps(pairs: &[(u16, u16)]) -> SourceCapabilities {
        SourceCapabilities::new_with_pdos(
            pairs
                .iter()
                .map(|&(mv, ma)| {
                    PowerDataObject::FixedSupply(
                        FixedSupply(0)
                            .with_raw_voltage(mv / 50)
                            .with_raw_max_current(ma / 10),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn selects_twenty_volts_with_correct_pdo_position() {
        let c = choose(&caps(&[(5000, 3000), (9000, 3000), (20000, 5000)]), None).unwrap();
        assert_eq!((c.mv, c.ma), (20000, 3000));
        let PowerSource::FixedVariableSupply(r) = c.request else {
            panic!()
        };
        assert_eq!(r.object_position(), 3);
        assert_eq!(r.operating_current().get::<milliampere>(), 3000);
        assert!(!r.usb_communications_capable());
    }

    #[test]
    fn respects_source_current_limit() {
        let c = choose(&caps(&[(5000, 3000), (20000, 1500)]), None).unwrap();
        assert_eq!((c.mv, c.ma), (20000, 1500));
    }

    #[test]
    fn spr_build_does_not_advertise_epr_entry_capability() {
        let mut c = caps(&[(5000, 3000), (15000, 3000)]);
        let pdos = c
            .pdos()
            .iter()
            .enumerate()
            .map(|(i, &p)| match p {
                PowerDataObject::FixedSupply(f) if i == 0 => {
                    PowerDataObject::FixedSupply(f.with_epr_mode_capable(true))
                }
                _ => p,
            })
            .collect();
        c = SourceCapabilities::new_with_pdos(pdos);
        let choice = choose(&c, Some(6200)).unwrap();
        let PowerSource::FixedVariableSupply(r) = choice.request else {
            panic!()
        };
        assert_eq!(r.epr_mode_capable(), EPR_ENABLED);
        assert_eq!(choice.mv, 15000);
    }

    #[test]
    fn cartridge_load_selects_a_lower_spr_voltage_before_heating() {
        let c = caps(&[(5000, 3000), (9000, 3000), (15000, 3000), (20000, 5000)]);
        let idle = choose(&c, None).unwrap();
        assert_eq!((idle.mv, idle.ma), (20000, 3000));
        let short_tip = choose(&c, Some(6200)).unwrap();
        assert_eq!((short_tip.mv, short_tip.ma), (15000, 3000));
        let long_tip = choose(&c, Some(8000)).unwrap();
        assert_eq!((long_tip.mv, long_tip.ma), (20000, 3000));
        let low_resistance = choose(&c, Some(4000)).unwrap();
        assert_eq!((low_resistance.mv, low_resistance.ma), (9000, 3000));
        let invalid = choose(&c, Some(0)).unwrap();
        assert_eq!(invalid.mv, 5000);
    }

    #[test]
    fn current_estimate_rounds_up_and_does_not_depend_on_average_power() {
        assert_eq!(heater_on_current_ma(28000, 6200), Some(4517));
        assert_eq!(heater_on_current_ma(15000, 6200), Some(2420));
        assert_eq!(heater_on_current_ma(28000, 0), None);
    }

    #[test]
    fn selects_intermediate_spr_but_never_an_spr_pdo_above_twenty_volts() {
        let c = choose(
            &caps(&[(5000, 1500), (9000, 3000), (15000, 3000), (28000, 5000)]),
            None,
        )
        .unwrap();
        assert_eq!((c.mv, c.ma), (15000, 3000));
    }

    #[test]
    fn refuses_malformed_or_unpowered_capabilities() {
        assert!(choose(&caps(&[]), None).is_none());
        assert!(choose(&caps(&[(9000, 3000), (20000, 3000)]), None).is_none());
        assert!(choose(&caps(&[(5000, 0), (20000, 3000)]), None).is_none());
    }

    fn epr_caps(epr_mv: u16, epr_ma: u16) -> SourceCapabilities {
        let c = caps(&[
            (5000, 3000),
            (9000, 3000),
            (20000, 5000),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (epr_mv, epr_ma),
            (36000, 5000),
            (48000, 5000),
        ]);
        SourceCapabilities::new_with_pdos(
            c.pdos()
                .iter()
                .enumerate()
                .map(|(i, &pdo)| {
                    if i == 0
                        && let PowerDataObject::FixedSupply(f) = pdo
                    {
                        PowerDataObject::FixedSupply(f.with_epr_mode_capable(true))
                    } else {
                        pdo
                    }
                })
                .collect(),
        )
    }

    #[test]
    fn selects_epr_twenty_eight_with_position_eight_and_the_exact_pdo_copy() {
        let caps = epr_caps(28000, 5000);
        let c = choose(&caps, None).unwrap();
        assert_eq!((c.mv, c.ma), (28000, 5000));
        assert_eq!(c.request.num_objects(), 2);
        let PowerSource::EprRequest(epr) = c.request else {
            panic!()
        };
        let rdo = FixedVariableSupply(epr.rdo);
        assert_eq!(rdo.object_position(), 8);
        assert_eq!(epr.pdo, caps.pdos()[7]);
        assert_eq!(rdo.operating_current().get::<milliampere>(), 5000);
        assert!(rdo.epr_mode_capable());
        assert!(rdo.no_usb_suspend());
        assert!(!rdo.usb_communications_capable());
        assert!(!rdo.unchunked_extended_messages_supported());
        assert_eq!(EPR_OPERATIONAL_WATTS, 140);
    }

    #[test]
    fn epr_limits_current_to_the_offer_and_never_chooses_above_twenty_eight() {
        let c = choose(&epr_caps(28000, 1500), None).unwrap();
        assert_eq!((c.mv, c.ma), (28000, 1500));
        let c = choose(&epr_caps(36000, 5000), None).unwrap();
        assert_eq!((c.mv, c.ma), (20000, 5000));
        let PowerSource::EprRequest(epr) = c.request else {
            panic!()
        };
        assert_eq!(epr.object_position(), 3);
    }

    #[test]
    fn epr_cartridge_load_requires_the_offered_five_amp_contract() {
        let full = choose(&epr_caps(28000, 5000), Some(6200)).unwrap();
        assert_eq!((full.mv, full.ma), (28000, 5000));
        let limited = choose(&epr_caps(28000, 4500), Some(6200)).unwrap();
        assert_eq!((limited.mv, limited.ma), (20000, 5000));
        let low = choose(&epr_caps(28000, 3000), Some(4000)).unwrap();
        assert_eq!((low.mv, low.ma), (9000, 3000));
    }

    #[test]
    fn unpowered_epr_pdo_falls_back_with_a_valid_two_object_epr_request() {
        let caps = epr_caps(28000, 0);
        let c = choose(&caps, None).unwrap();
        assert_eq!(c.mv, 20000);
        let PowerSource::EprRequest(epr) = c.request else {
            panic!()
        };
        assert_eq!(epr.pdo, caps.pdos()[2]);
    }

    #[test]
    fn initial_spr_request_advertises_epr_only_when_the_source_does() {
        let capable = SourceCapabilities::new_with_pdos(
            epr_caps(28000, 5000).pdos()[..3].iter().copied().collect(),
        );
        let c = choose(&capable, None).unwrap();
        assert_eq!(c.mv, 20000);
        let PowerSource::FixedVariableSupply(r) = c.request else {
            panic!()
        };
        assert_eq!(r.epr_mode_capable(), EPR_ENABLED);
        let c = choose(&caps(&[(5000, 3000), (20000, 5000)]), None).unwrap();
        let PowerSource::FixedVariableSupply(r) = c.request else {
            panic!()
        };
        assert!(!r.epr_mode_capable());
    }
}
