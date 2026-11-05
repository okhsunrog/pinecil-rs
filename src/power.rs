use uom::si::{electric_current::milliampere, electric_potential::millivolt};
use usbpd::protocol_layer::message::data::{
    request::{EprRequestDataObject, FixedVariableSupply, PowerSource},
    source_capabilities::{PowerDataObject, SourceCapabilities},
};

pub const TARGET_MV: u32 = 20_000;
pub const EPR_TARGET_MV: u32 = 28_000;
pub const MAX_CURRENT_MA: u32 = 3_000;
pub const EPR_OPERATIONAL_WATTS: u32 = EPR_TARGET_MV * MAX_CURRENT_MA / 1_000_000;

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

/// Prefer fixed 28 V only after receiving EPR capabilities. Initial SPR
/// negotiation and fallback prefer 20 V, then 5 V. Never select PPS/AVS or >28 V.
pub fn choose(caps: &SourceCapabilities) -> Option<Choice> {
    let safe = caps.vsafe_5v()?;
    if safe.voltage().get::<millivolt>() != 5_000 || safe.max_current().get::<milliampere>() == 0 {
        return None;
    }
    let epr = caps.is_epr_capabilities();
    let fixed_at = |(position, pdo): (u8, &PowerDataObject), target| match pdo {
        PowerDataObject::FixedSupply(f)
            if f.voltage().get::<millivolt>() == target
                && f.max_current().get::<milliampere>() > 0 =>
        {
            Some((position, *pdo))
        }
        _ => None,
    };
    let selected_epr = if epr {
        caps.epr_pdos().find_map(|pdo| fixed_at(pdo, EPR_TARGET_MV))
    } else {
        None
    };
    let (position, pdo) = selected_epr
        .or_else(|| caps.spr_pdos().find_map(|pdo| fixed_at(pdo, TARGET_MV)))
        .unwrap_or((1, PowerDataObject::FixedSupply(*safe)));
    let PowerDataObject::FixedSupply(selected) = pdo else {
        unreachable!()
    };
    let mv = selected.voltage().get::<millivolt>();
    let ma = selected
        .max_current()
        .get::<milliampere>()
        .min(MAX_CURRENT_MA);
    let request = FixedVariableSupply(0)
        .with_object_position(position)
        .with_raw_operating_current((ma / 10) as u16)
        .with_raw_max_operating_current((ma / 10) as u16)
        .with_no_usb_suspend(true)
        .with_usb_communications_capable(false)
        .with_epr_mode_capable(epr || caps.epr_mode_capable());
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
        let c = choose(&caps(&[(5000, 3000), (9000, 3000), (20000, 5000)])).unwrap();
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
        let c = choose(&caps(&[(5000, 3000), (20000, 1500)])).unwrap();
        assert_eq!((c.mv, c.ma), (20000, 1500));
    }

    #[test]
    fn falls_back_to_five_instead_of_intermediate_or_epr_voltage() {
        let c = choose(&caps(&[
            (5000, 1500),
            (9000, 3000),
            (15000, 3000),
            (28000, 5000),
        ]))
        .unwrap();
        assert_eq!((c.mv, c.ma), (5000, 1500));
    }

    #[test]
    fn refuses_malformed_or_unpowered_capabilities() {
        assert!(choose(&caps(&[])).is_none());
        assert!(choose(&caps(&[(9000, 3000), (20000, 3000)])).is_none());
        assert!(choose(&caps(&[(5000, 0), (20000, 3000)])).is_none());
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
        let c = choose(&caps).unwrap();
        assert_eq!((c.mv, c.ma), (28000, 3000));
        assert_eq!(c.request.num_objects(), 2);
        let PowerSource::EprRequest(epr) = c.request else {
            panic!()
        };
        let rdo = FixedVariableSupply(epr.rdo);
        assert_eq!(rdo.object_position(), 8);
        assert_eq!(epr.pdo, caps.pdos()[7]);
        assert_eq!(rdo.operating_current().get::<milliampere>(), 3000);
        assert!(rdo.epr_mode_capable());
        assert!(rdo.no_usb_suspend());
        assert!(!rdo.usb_communications_capable());
        assert!(!rdo.unchunked_extended_messages_supported());
        assert_eq!(EPR_OPERATIONAL_WATTS, 84);
    }

    #[test]
    fn epr_limits_current_to_the_offer_and_never_chooses_above_twenty_eight() {
        let c = choose(&epr_caps(28000, 1500)).unwrap();
        assert_eq!((c.mv, c.ma), (28000, 1500));
        let c = choose(&epr_caps(36000, 5000)).unwrap();
        assert_eq!((c.mv, c.ma), (20000, 3000));
        let PowerSource::EprRequest(epr) = c.request else {
            panic!()
        };
        assert_eq!(epr.object_position(), 3);
    }

    #[test]
    fn unpowered_epr_pdo_falls_back_with_a_valid_two_object_epr_request() {
        let caps = epr_caps(28000, 0);
        let c = choose(&caps).unwrap();
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
        let c = choose(&capable).unwrap();
        assert_eq!(c.mv, 20000);
        let PowerSource::FixedVariableSupply(r) = c.request else {
            panic!()
        };
        assert!(r.epr_mode_capable());
        let c = choose(&caps(&[(5000, 3000), (20000, 5000)])).unwrap();
        let PowerSource::FixedVariableSupply(r) = c.request else {
            panic!()
        };
        assert!(!r.epr_mode_capable());
    }
}
