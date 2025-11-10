#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use core::cell::RefCell;
use core::num::TryFromIntError;

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::encode_into_slice;
use bincode::error::EncodeError;
use common_pico::{
    HEADER_SIZE, REQUEST_MAX_SIZE, SERVER_IP_ADDRESS, SERVER_TCP_PORT_SENSORS,
    connect_loco_controller, initialize_logger, initialize_program, initialize_wifi,
};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Delay, Instant, Timer};
use embedded_hal_bus::spi::RefCellDevice;
use embedded_io_async::Write as _;
use heapless::Vec;
use loco_protocol::{
    BACKEND_PROTOCOL_MAGIC_NUMBER, Header, LocoId, Operation, SensorId, SensorStatus,
    SensorsStatusArray,
};
use mfrc522::comm::blocking::spi::{DummyDelay, SpiInterface};
use mfrc522::{Initialized, Mfrc522, RxGain, Uid};
use {defmt_rtt as _, panic_probe as _};

struct RfidReader<'a> {
    mfrc522: Mfrc522<
        SpiInterface<
            RefCellDevice<'a, Spi<'static, SPI0, Blocking>, Output<'static>, Delay>,
            DummyDelay,
        >,
        Initialized,
    >,
    sensor_id: SensorId,
    sensor_data_idx: usize,
}

struct SensorData {
    loco_id: LocoId,
    sensor_id: SensorId,
}

type SensorsData = [Option<SensorData>; 8];
static SENSORS_DATA: Mutex<CriticalSectionRawMutex, RefCell<SensorsData>> =
    Mutex::new(RefCell::new([
        None, None, None, None, None, None, None, None,
    ]));

#[embassy_executor::task]
async fn tag_reader_task(
    spi: Spi<'static, SPI0, Blocking>,
    sensors_data: [(Output<'static>, SensorId); 8],
) {
    let spi_rc = RefCell::new(spi);
    let mut readers: Vec<RfidReader, 8> = Vec::new();
    let mut sensor_data_idx: usize = 0;

    for (cs_pin, sensor_id) in sensors_data {
        let mut mfrc522 = Mfrc522::new(SpiInterface::new(RefCellDevice::new(
            &spi_rc, cs_pin, Delay,
        )))
        .init()
        .expect("could not create reader");
        mfrc522.set_receive_timeout(1).unwrap();
        mfrc522.set_antenna_gain(RxGain::DB48).unwrap();

        if let Err(reader) = readers.push(RfidReader {
            mfrc522,
            sensor_id,
            sensor_data_idx,
        }) {
            log::error!("Readers vector is full, can't add {:?}", reader.sensor_id);
        };

        sensor_data_idx += 1;
    }

    loop {
        for reader in readers.iter_mut() {
            if let Ok(atqa) = reader.mfrc522.wupa() {
                match reader.mfrc522.select(&atqa) {
                    Ok(Uid::Single(ref uid)) => match LocoId::try_from(uid.as_bytes()) {
                        Ok(loco_id) => {
                            log::debug!("[{}] Detected {}", reader.sensor_id, loco_id);
                            SENSORS_DATA.lock(|d| {
                                d.borrow_mut()[reader.sensor_data_idx] = Some(SensorData {
                                    loco_id,
                                    sensor_id: reader.sensor_id,
                                })
                            });
                        }
                        Err(e) => log::error!("[{}] Invalid UID: {:?}", reader.sensor_id, e),
                    },
                    Ok(_) => log::debug!("[{}] Got other UID size", reader.sensor_id),
                    Err(e) => {
                        log::debug!("[{}] Error getting card UID: {:?}", reader.sensor_id, e);
                    }
                }
                let _ = reader.mfrc522.hlta();
            }
        }

        Timer::after_millis(1).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    initialize_logger(&spawner, p.USB);
    initialize_program("SensorsPico").await;
    let (mut control, stack) = initialize_wifi(
        &spawner, p.PIN_23, p.PIN_25, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0,
    )
    .await;

    unwrap!(spawner.spawn(tag_reader_task(
        Spi::new_blocking(p.SPI0, p.PIN_2, p.PIN_3, p.PIN_4, spi::Config::default()),
        [
            (Output::new(p.PIN_10, Level::High), SensorId::RfidReader1),
            (Output::new(p.PIN_11, Level::High), SensorId::RfidReader2),
            (Output::new(p.PIN_12, Level::High), SensorId::RfidReader3),
            (Output::new(p.PIN_13, Level::High), SensorId::RfidReader4),
            (Output::new(p.PIN_18, Level::High), SensorId::RfidReader5),
            (Output::new(p.PIN_19, Level::High), SensorId::RfidReader6),
            (Output::new(p.PIN_20, Level::High), SensorId::RfidReader7),
            (Output::new(p.PIN_21, Level::High), SensorId::RfidReader8),
        ],
    )));

    let sensors = Sensors::new();

    // Spawn a dedicated task that periodically read from all RFID readers

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    control.gpio_set(0, false).await;

    loop {
        let mut socket = match connect_loco_controller(
            stack,
            &mut rx_buffer,
            &mut tx_buffer,
            SERVER_IP_ADDRESS,
            SERVER_TCP_PORT_SENSORS,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                log::warn!("connection error: {:?}", e);
                Timer::after_secs(1).await;
                continue;
            }
        };

        control.gpio_set(0, true).await;

        // Periodically check sensors status and send updated status to
        // loco_controller
        if let Err(e) = sensors.handle_sensors_updates(&mut socket).await {
            log::error!("{:?}", e);
            continue;
        }

        control.gpio_set(0, false).await;
    }
}

#[derive(Debug)]
pub enum Error {
    EncodeIntoSlice(EncodeError),
    InvalidEncodedHeaderSize(usize),
    PayloadSizeTooLarge(TryFromIntError),
    TcpWrite(embassy_net::tcp::Error),
}

type Result<T> = core::result::Result<T, Error>;

struct Sensors {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
}

impl Sensors {
    pub fn new() -> Self {
        log::debug!("Sensors::new()");

        Sensors {
            bincode_cfg: bincode::config::legacy(),
        }
    }

    fn extend_payload_with_sensor_status_list(&self, payload: &mut [u8]) -> Result<(u8, u8)> {
        log::debug!("Sensors::extend_payload_with_sensor_status_list()");

        let mut payload_offset: usize = size_of::<SensorsStatusArray>();
        let mut updated_sensors: u8 = 0;
        SENSORS_DATA.lock(|d| {
            let mut sensors_data = d.borrow_mut();
            for sensor_data in sensors_data.iter_mut() {
                if let Some(d) = sensor_data.take() {
                    log::info!("{} detected by reader {}", d.loco_id, d.sensor_id);
                    payload_offset += encode_into_slice(
                        SensorStatus {
                            sensor_id: d.sensor_id.into(),
                            loco_id: d.loco_id.into(),
                        },
                        &mut payload[payload_offset..],
                        self.bincode_cfg,
                    )
                    .unwrap();
                    updated_sensors += 1;
                }
            }
        });

        Ok((
            updated_sensors,
            u8::try_from(payload_offset).map_err(Error::PayloadSizeTooLarge)?,
        ))
    }

    fn extend_payload_with_sensors_status_array(
        &self,
        payload: &mut [u8],
        updated_sensors: u8,
    ) -> Result<()> {
        log::debug!("Sensors::extend_payload_with_sensors_status_array()");
        encode_into_slice(
            SensorsStatusArray {
                len: updated_sensors,
            },
            &mut payload[0..],
            self.bincode_cfg,
        )
        .map_err(Error::EncodeIntoSlice)?;

        Ok(())
    }

    async fn send_sensors_status_op(
        &self,
        socket: &mut TcpSocket<'_>,
        message: &mut [u8],
        payload_len: u8,
    ) -> Result<()> {
        log::debug!("Sensors::send_sensors_status_op()");

        let header_len = encode_into_slice(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::SensorsStatus.into(),
                payload_len,
            },
            &mut message[..HEADER_SIZE],
            self.bincode_cfg,
        )
        .map_err(Error::EncodeIntoSlice)?;

        if header_len != HEADER_SIZE {
            return Err(Error::InvalidEncodedHeaderSize(header_len));
        }

        socket
            .write_all(&message[..header_len + usize::from(payload_len)])
            .await
            .map_err(Error::TcpWrite)?;

        Ok(())
    }

    pub async fn handle_sensors_updates(&self, socket: &mut TcpSocket<'_>) -> Result<()> {
        log::debug!("Sensors::handle_sensors_updates()");

        let mut message = [0u8; REQUEST_MAX_SIZE];
        let payload_offset = HEADER_SIZE;
        let mut now = Instant::now();

        loop {
            // Check sensors which need to be updated and fill payload
            let (updated_sensors, payload_len) =
                self.extend_payload_with_sensor_status_list(&mut message[payload_offset..])?;

            // Communicate with the loco_controller every second, even if no
            // sensor was updated. This maintains the connection alive at a
            // very minimal cost.
            if updated_sensors > 0 || now.elapsed().as_millis() > 1000 {
                self.extend_payload_with_sensors_status_array(
                    &mut message[payload_offset..],
                    updated_sensors,
                )?;

                // Send update to the loco_controller server
                self.send_sensors_status_op(socket, &mut message, payload_len)
                    .await?;

                // Update timer
                now = Instant::now();
            }

            Timer::after_millis(100).await;
        }
    }
}
