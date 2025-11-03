#![no_std]

use embassy_net::IpAddress;

/**
 * Constants related to the WiFi connection between the Pi Pico boards
 * and the main controller.
 */
pub const WIFI_NETWORK: &str = "loco-controller";
pub const WIFI_PASSWORD: &str = "locoloco";
pub const SERVER_IP_ADDRESS: IpAddress = IpAddress::v4(10, 42, 0, 1);
pub const SERVER_TCP_PORT_LOCOS: u16 = 8004;
pub const SERVER_TCP_PORT_SENSORS: u16 = 8005;
pub const SERVER_TCP_PORT_ACTUATORS: u16 = 8006;

/**
 * Constants related to the protocol, but specific to the Pi Pico constraints.
 */
pub const PAYLOAD_MAX_SIZE: usize = 256;
pub const HEADER_SIZE: usize = 0x3;
pub const REQUEST_MAX_SIZE: usize = HEADER_SIZE + PAYLOAD_MAX_SIZE;
pub const RESPONSE_MAX_SIZE: usize = 1024;
