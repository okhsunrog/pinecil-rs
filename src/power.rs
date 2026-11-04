use uom::si::{electric_current::milliampere, electric_potential::millivolt};
use usbpd::{
    protocol_layer::message::data::{
        request::{CurrentRequest, PowerSource, VoltageRequest},
        source_capabilities::{PowerDataObject, SourceCapabilities},
    },
    units::{ElectricCurrent, ElectricPotential},
};

pub const TARGET_MV: u32 = 20_000;
pub const MAX_CURRENT_MA: u32 = 3_000;

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

/// Require the mandatory first 5 V PDO and never select PPS, EPR, or an
/// intermediate voltage. Limit the request to both the source and board limit.
pub fn choose(caps: &SourceCapabilities) -> Option<Choice> {
    let safe = caps.vsafe_5v()?;
    if safe.voltage().get::<millivolt>() != 5_000 || safe.max_current().get::<milliampere>() == 0 {
        return None;
    }
    let selected = caps
        .spr_pdos()
        .find_map(|(_, pdo)| match pdo {
            PowerDataObject::FixedSupply(f)
                if f.voltage().get::<millivolt>() == TARGET_MV
                    && f.max_current().get::<milliampere>() > 0 =>
            {
                Some(f)
            }
            _ => None,
        })
        .unwrap_or(safe);
    let mv = selected.voltage().get::<millivolt>();
    let ma = selected
        .max_current()
        .get::<milliampere>()
        .min(MAX_CURRENT_MA);
    let PowerSource::FixedVariableSupply(mut request) = PowerSource::new_fixed(
        CurrentRequest::Specific(ElectricCurrent::new::<milliampere>(ma)),
        VoltageRequest::Specific(ElectricPotential::new::<millivolt>(mv)),
        caps,
    )
    .ok()?
    else {
        return None;
    };
    // The application currently exposes no USB data interface.
    request.set_usb_communications_capable(false);
    Some(Choice {
        request: PowerSource::FixedVariableSupply(request),
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
}
