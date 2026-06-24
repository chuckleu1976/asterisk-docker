use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub enum SmsType {
    RecUnread,
    RecRead,
    StoUnsent,
    StoSent,
    All,
}

impl SmsType {
    pub fn to_at_command_pdu(&self) -> u8 {
        match self {
            SmsType::RecUnread => 0,
            SmsType::RecRead => 1,
            SmsType::StoUnsent => 2,
            SmsType::StoSent => 3,
            SmsType::All => 4,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRegistrationStatus {
    pub status: String,
    pub location_area_code: Option<String>,
    pub cell_id: Option<String>,
}

impl NetworkRegistrationStatus {
    fn parse_from_line(line: &str) -> Option<Self> {
        let data = line.split(':').nth(1)?;
        let parts: Vec<&str> = data.split(',').collect();
        if parts.len() >= 2 {
            Some(NetworkRegistrationStatus {
                status: parts[1].trim().trim_matches('"').to_string(),
                location_area_code: parts
                    .get(2)
                    .map(|s| s.trim().trim_matches('"').to_string()),
                cell_id: parts.get(3).map(|s| s.trim().trim_matches('"').to_string()),
            })
        } else if parts.len() == 1 {
            // Some modems return +CREG: <stat> (single value, mode=0)
            Some(NetworkRegistrationStatus {
                status: parts[0].trim().trim_matches('"').to_string(),
                location_area_code: None,
                cell_id: None,
            })
        } else {
            None
        }
    }

    /// Parse AT+CREG? response (CS domain — 2G/3G)
    pub fn from_response(response: &str) -> Option<Self> {
        response
            .lines()
            .find(|line| line.trim().starts_with("+CREG:"))
            .and_then(|line| Self::parse_from_line(line))
    }

    /// Parse AT+CEREG? response (EPS domain — LTE)
    pub fn from_cereg_response(response: &str) -> Option<Self> {
        response
            .lines()
            .find(|line| line.trim().starts_with("+CEREG:"))
            .and_then(|line| Self::parse_from_line(line))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInfo {
    pub operator_name: String,
    pub operator_id: String,
    pub registration_status: String,
}

impl OperatorInfo {
    pub fn from_response(response: &str) -> Option<Self> {
        response
            .lines()
            .find(|line| line.trim().starts_with("+COPS:"))
            .and_then(|line| {
                let data = line.split(':').nth(1)?;
                let parts: Vec<&str> = data.split(',').collect();

                if parts.len() >= 3 {
                    Some(OperatorInfo {
                        registration_status: parts[0].trim().to_string(),
                        operator_name: parts[2].trim_matches('"').to_string(),
                        operator_id: parts
                            .get(3)
                            .map(|s| s.trim_matches('"').to_string())
                            .unwrap_or_else(|| parts[2].trim_matches('"').to_string()),
                    })
                } else {
                    None
                }
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModemInfo {
    model: String,
}

impl ModemInfo {
    pub fn from_response(response: &str) -> Option<Self> {
        let model = response.trim().to_string();
        if !model.is_empty() && !model.contains("ERROR") {
            Some(ModemInfo { model })
        } else {
            None
        }
    }
}
