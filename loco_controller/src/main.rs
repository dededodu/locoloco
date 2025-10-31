use actix_web::{
    App, HttpResponse, HttpServer, Responder, body::BoxBody, get, http::StatusCode, post, web,
};
use clap::Parser;
use loco_protocol::{ActuatorId, ActuatorType, Direction, LocoId, Speed, SwitchRailsState};
use log::{debug, error};
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::TcpListener,
    sync::Arc,
    thread::{self, sleep},
    time::Duration,
};
use thiserror::Error;

mod backend;
mod oracle;
mod rail_network;
use crate::{
    backend::{Backend, LocoIntent, OracleMode},
    oracle::Oracle,
};

#[derive(Debug, Error)]
enum Error {
    #[error("Error binding listener {0}")]
    BindListener(#[source] io::Error),
    #[error("Error running HTTP server {0}")]
    HttpServer(#[source] io::Error),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
struct ControlLocoParams {
    loco_id: LocoId,
    direction: Direction,
    speed: Speed,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
struct LocoIntentParams {
    loco_id: LocoId,
    loco_intent: LocoIntent,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
struct DriveSwitchRailsParams {
    actuator_id: ActuatorId,
    state: SwitchRailsState,
}

#[get("/")]
async fn index(_data: web::Data<Arc<Backend>>) -> impl Responder {
    HttpResponse::Ok().body("Loco controller running!")
}

#[get("/loco_status/{loco_id}")]
async fn loco_status(path: web::Path<LocoId>, data: web::Data<Arc<Backend>>) -> impl Responder {
    let loco_id = path.into_inner();

    match data.loco_status(loco_id) {
        Ok(status) => HttpResponse::Ok().json(status),
        Err(e) => {
            error!("{}", e);
            HttpResponse::with_body(
                StatusCode::INTERNAL_SERVER_ERROR,
                BoxBody::new(format!("{}", e)),
            )
        }
    }
}

#[post("/control_loco")]
async fn control_loco(
    form: web::Json<ControlLocoParams>,
    data: web::Data<Arc<Backend>>,
) -> impl Responder {
    if data.oracle_enabled() {
        let e = "Oracle is running, can't manually control the loco";
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    if let Err(e) = data.control_loco(form.loco_id, form.direction, form.speed) {
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    HttpResponse::Ok().body(format!(
        "Move {:?} loco {:?} at {:?} speed",
        form.direction, form.loco_id, form.speed
    ))
}

#[post("/loco_intent")]
async fn loco_intent(
    form: web::Json<LocoIntentParams>,
    data: web::Data<Arc<Backend>>,
) -> impl Responder {
    data.set_loco_intent(form.loco_id, form.loco_intent);
    HttpResponse::Ok().body(format!(
        "Setting loco intent {:?} for {:?}",
        form.loco_intent, form.loco_id
    ))
}

#[post("/drive_switch_rails")]
async fn drive_switch_rails(
    form: web::Json<DriveSwitchRailsParams>,
    data: web::Data<Arc<Backend>>,
) -> impl Responder {
    if data.oracle_enabled() {
        let e = "Oracle is running, can't manually drive switch rails";
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    if let Err(e) = data.drive_actuator(
        form.actuator_id,
        ActuatorType::SwitchRails,
        form.state.into(),
    ) {
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    HttpResponse::Ok().body(format!("Drive {:?} to {:?}", form.actuator_id, form.state))
}

#[post("/oracle_mode")]
async fn oracle_mode(form: web::Json<OracleMode>, data: web::Data<Arc<Backend>>) -> impl Responder {
    data.set_oracle_mode(form.0);
    HttpResponse::Ok().body(format!("Setting Oracle to mode {:?}", form.0))
}

#[actix_web::main]
async fn http_main(port: u16, backend: Arc<Backend>) -> std::io::Result<()> {
    debug!("http_main(): Waiting for incoming connection...");
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(backend.clone()))
            .service(index)
            .service(loco_status)
            .service(control_loco)
            .service(loco_intent)
            .service(drive_switch_rails)
            .service(oracle_mode)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

fn backend_locos(port: u16, backend: Arc<Backend>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_locos(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("backend_locos(): Connected");
        if let Err(e) = backend.handle_loco_connection(stream) {
            error!("{}", e);
        }
    }
}

fn backend_sensors(port: u16, backend: Arc<Backend>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_sensors(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("backend_sensors(): Connected");
        if let Err(e) = backend.serve_sensors(stream) {
            error!("{}", e);
        }
    }
}

fn backend_actuators(port: u16, backend: Arc<Backend>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_actuators(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("backend_actuators(): Connected");
        if let Err(e) = backend.handle_actuators_connection(stream) {
            error!("{}", e);
        }
    }
}

fn backend_oracle(backend: Arc<Backend>) -> Result<()> {
    debug!("backend_oracle()");
    let mut oracle = Oracle::new(backend);
    loop {
        if let Err(e) = oracle.process() {
            error!("{}", e);
        }
        sleep(Duration::from_millis(10));
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 8080)]
    http_port: u16,
    #[arg(long, default_value_t = 8004)]
    backend_locos_port: u16,
    #[arg(long, default_value_t = 8005)]
    backend_sensors_port: u16,
    #[arg(long, default_value_t = 8006)]
    backend_actuators_port: u16,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();

    // Initialize backend
    let backend = Arc::new(Backend::new());
    let shared_backend_locos = backend.clone();
    let shared_backend_sensors = backend.clone();
    let shared_backend_actuators = backend.clone();
    let shared_backend_oracle = backend.clone();

    // Start backend server, waiting for incoming connections from locos
    thread::spawn(move || backend_locos(args.backend_locos_port, shared_backend_locos));

    // Start backend server, waiting for updates on locos' positions
    thread::spawn(move || backend_sensors(args.backend_sensors_port, shared_backend_sensors));

    // Start backend server, waiting for incoming connection from actuators
    thread::spawn(move || backend_actuators(args.backend_actuators_port, shared_backend_actuators));

    // Start railway network automation process
    thread::spawn(move || backend_oracle(shared_backend_oracle));

    http_main(args.http_port, backend).map_err(Error::HttpServer)?;

    Ok(())
}
