pub mod core;
#[cfg(not(feature = "mock-data"))]
pub mod manager;
#[cfg(feature = "mock-data")]
pub mod mock_manager;
pub mod ami;
pub mod ami_transport;
pub mod pdu;
pub mod transport;
pub mod types;

#[cfg(not(feature = "mock-data"))]
pub use manager::ModemManager;
#[cfg(feature = "mock-data")]
pub use mock_manager::ModemManager;
pub use transport::{ModemEvent, SimInfo, Transport};
pub use types::{ModemInfo, NetworkRegistrationStatus, OperatorInfo, SignalQuality, SmsType};
