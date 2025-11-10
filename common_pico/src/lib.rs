#![no_std]

use cyw43::{Control, JoinOptions};
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::{ConnectError, TcpSocket};
use embassy_net::{Config, IpAddress, IpEndpoint, Stack, StackResources};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output, Pin};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio, PioPin};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_rp::{Peri, bind_interrupts};
use embassy_time::Timer;
use rand::RngCore;
use static_cell::StaticCell;

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

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

#[embassy_executor::task]
pub async fn logger_task(driver: UsbDriver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::task]
pub async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

pub fn initialize_logger(spawner: &Spawner, usb: Peri<'static, USB>) {
    let usb_driver = UsbDriver::new(usb, Irqs);
    unwrap!(spawner.spawn(logger_task(usb_driver)));
}

pub async fn initialize_program(program_name: &str) {
    let mut counter = 0;
    while counter < 10 {
        counter += 1;
        log::debug!("Tick {}", counter);
        Timer::after_secs(1).await;
    }
    log::info!("Hello {}!", program_name);
}

pub async fn initialize_wifi<'a, 'b>(
    spawner: &Spawner,
    pwr_pin: Peri<'static, impl Pin>,
    cs_pin: Peri<'static, impl Pin>,
    pio_pin: Peri<'static, PIO0>,
    dio: Peri<'static, impl PioPin>,
    clk: Peri<'static, impl PioPin>,
    dma: Peri<'static, DMA_CH0>,
) -> (Control<'a>, Stack<'b>) {
    let fw = include_bytes!("../../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../../cyw43-firmware/43439A0_clm.bin");

    let pwr = Output::new(pwr_pin, Level::Low);
    let cs = Output::new(cs_pin, Level::High);
    let mut pio = Pio::new(pio_pin, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        dio,
        clk,
        dma,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(cyw43_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::None)
        .await;

    let config = Config::dhcpv4(Default::default());

    // Generate random seed
    let mut rng = RoscRng;
    let seed = rng.next_u64();

    // Init network stack
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    unwrap!(spawner.spawn(net_task(runner)));

    loop {
        match control
            .join(WIFI_NETWORK, JoinOptions::new(WIFI_PASSWORD.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(err) => {
                log::error!("join failed with status={}", err.status);
            }
        }
    }

    // Wait for DHCP
    log::info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_secs(1).await;
    }
    log::info!("DHCP is now up!");

    (control, stack)
}

pub async fn connect_loco_controller<'a>(
    stack: Stack<'a>,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    addr: IpAddress,
    port: u16,
) -> Result<TcpSocket<'a>, ConnectError> {
    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);

    let remote_endpoint = IpEndpoint { addr, port };

    log::info!("Connecting to {:?}...", remote_endpoint);
    socket.connect(remote_endpoint).await?;
    log::info!("Connected to {:?}", socket.remote_endpoint());

    Ok(socket)
}
