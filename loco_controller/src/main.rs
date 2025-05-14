use actix_web::{
    App, HttpResponse, HttpServer, Responder, body::BoxBody, get, http::StatusCode, post, web,
};
use bincode::{
    config::{Configuration, Fixint, LittleEndian, NoLimit},
    decode_from_std_read, encode_to_vec,
    error::{DecodeError, EncodeError},
};
use clap::Parser;
use loco_protocol::{
    ConnectPayload, ControlLoco, ControlLocoPayload, Direction, Error as LocoProtocolError, Header,
    LocoId, LocoStatus, LocoStatusResponse, Operation, Speed,
};
use log::{debug, error};
use std::{
    collections::HashMap,
    io::{self, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};
use thiserror::Error;

const BACKEND_PROTOCOL_MAGIC_NUMBER: u8 = 0xab;

#[derive(Debug, Error)]
enum Error {
    #[error("Error binding listener {0}")]
    BindListener(#[source] io::Error),
    #[error("Error converting into expected type")]
    ConvertLocoProtocolType(LocoProtocolError),
    #[error("Error decoding from TCP stream: {0}")]
    DecodeFromStream(#[source] DecodeError),
    #[error("Error encoding to vec: {0}")]
    EncodeToVec(#[source] EncodeError),
    #[error("Error running HTTP server {0}")]
    HttpServer(#[source] io::Error),
    #[error("Invalid backend protocol magic number {0}")]
    InvalidBackendProtocolMagicNumber(u8),
    #[error("Loco {0} not connected")]
    LocoNotConnected(LocoId),
    #[error("Unsupported operation {0}")]
    UnsupportedOperation(Operation),
    #[error("Error writing to TCP stream {0}")]
    WriteTcpStream(#[source] io::Error),
}

type Result<T> = std::result::Result<T, Error>;

struct LocoInfo {
    stream: TcpStream,
}

struct Backend {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
    loco_info: HashMap<LocoId, LocoInfo>,
}

impl Backend {
    fn new() -> Self {
        debug!("Backend::new()");

        let bincode_cfg = bincode::config::legacy();
        let loco_info = HashMap::new();

        Backend {
            bincode_cfg,
            loco_info,
        }
    }

    fn handle_op_connect(&mut self, mut stream: TcpStream) -> Result<()> {
        debug!("Backend::handle_op_connect()");

        // Retrieve payload
        let payload: ConnectPayload = decode_from_std_read(&mut stream, self.bincode_cfg.clone())
            .map_err(Error::DecodeFromStream)?;
        let loco_id = LocoId::try_from(payload.loco_id).map_err(Error::ConvertLocoProtocolType)?;
        debug!("Backend::handle_op_connect(): LocoId {:?}", loco_id);

        self.loco_info.insert(loco_id, LocoInfo { stream });

        Ok(())
    }

    fn handle_connection(&mut self, mut stream: TcpStream) -> Result<()> {
        debug!("Backend::handle_connection()");

        // Retrieve header
        let header: Header = decode_from_std_read(&mut stream, self.bincode_cfg.clone())
            .map_err(Error::DecodeFromStream)?;

        debug!("Backend::handle_connection(): {:?}", header);

        if header.magic != BACKEND_PROTOCOL_MAGIC_NUMBER {
            return Err(Error::InvalidBackendProtocolMagicNumber(header.magic));
        }

        let op = Operation::try_from(header.operation).map_err(Error::ConvertLocoProtocolType)?;
        debug!("Backend::handle_connection(): Operation {:?}", op);

        match op {
            Operation::Connect => self.handle_op_connect(stream)?,
            Operation::ControlLoco | Operation::LocoStatus => {
                return Err(Error::UnsupportedOperation(op));
            }
        }

        Ok(())
    }

    fn control_loco(&mut self, loco_id: LocoId, direction: Direction, speed: Speed) -> Result<()> {
        debug!(
            "Backend::control_loco(): loco_id {:?}, direction {:?}, speed {:?}",
            loco_id, direction, speed
        );

        let loco_info = self
            .loco_info
            .get_mut(&loco_id)
            .ok_or(Error::LocoNotConnected(loco_id))?;

        let mut payload = encode_to_vec(
            ControlLocoPayload {
                direction: direction.into(),
                speed: speed.into(),
            },
            self.bincode_cfg.clone(),
        )
        .map_err(Error::EncodeToVec)?;

        let mut message = encode_to_vec(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::ControlLoco.into(),
                payload_len: payload.len() as u8,
            },
            self.bincode_cfg.clone(),
        )
        .map_err(Error::EncodeToVec)?;

        message.append(&mut payload);

        loco_info
            .stream
            .write_all(message.as_slice())
            .map_err(Error::WriteTcpStream)?;

        Ok(())
    }

    fn loco_status(&mut self, loco_id: LocoId) -> Result<LocoStatus> {
        debug!("Backend::loco_status(): loco_id {:?}", loco_id);

        let loco_info = self
            .loco_info
            .get_mut(&loco_id)
            .ok_or(Error::LocoNotConnected(loco_id))?;

        let message = encode_to_vec(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::LocoStatus.into(),
                payload_len: 0,
            },
            self.bincode_cfg.clone(),
        )
        .map_err(Error::EncodeToVec)?;

        loco_info
            .stream
            .write_all(message.as_slice())
            .map_err(Error::WriteTcpStream)?;

        let resp: LocoStatusResponse =
            decode_from_std_read(&mut loco_info.stream, self.bincode_cfg.clone())
                .map_err(Error::DecodeFromStream)?;

        let status = LocoStatus {
            direction: Direction::try_from(resp.direction)
                .map_err(Error::ConvertLocoProtocolType)?,
            speed: Speed::try_from(resp.speed).map_err(Error::ConvertLocoProtocolType)?,
        };

        Ok(status)
    }
}

#[get("/")]
async fn index(_data: web::Data<Arc<Mutex<Backend>>>) -> impl Responder {
    HttpResponse::Ok().body("Loco controller running!")
}

#[get("/loco_status/{loco_id}")]
async fn loco_status(
    path: web::Path<LocoId>,
    data: web::Data<Arc<Mutex<Backend>>>,
) -> impl Responder {
    let loco_id = path.into_inner();

    match data.lock().unwrap().loco_status(loco_id) {
        Ok(status) => HttpResponse::Ok().json(status),
        Err(e) => {
            error!("{}", e);
            return HttpResponse::with_body(
                StatusCode::INTERNAL_SERVER_ERROR,
                BoxBody::new(format!("{}", e)),
            );
        }
    }
}

#[post("/control_loco")]
async fn control_loco(
    form: web::Json<ControlLoco>,
    data: web::Data<Arc<Mutex<Backend>>>,
) -> impl Responder {
    if let Err(e) = data
        .lock()
        .unwrap()
        .control_loco(form.loco_id, form.direction, form.speed)
    {
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

#[actix_web::main]
async fn http_main(port: u16, backend: Arc<Mutex<Backend>>) -> std::io::Result<()> {
    debug!("http_main(): Waiting for incoming connection...");
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(backend.clone()))
            .service(index)
            .service(loco_status)
            .service(control_loco)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

fn backend_main(port: u16, backend: Arc<Mutex<Backend>>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_main(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("main(): Connected");
        if let Err(e) = backend.lock().unwrap().handle_connection(stream) {
            error!("{}", e);
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 8080)]
    http_port: u16,
    #[arg(long, default_value_t = 8004)]
    backend_port: u16,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();

    // Initialize backend
    let backend = Arc::new(Mutex::new(Backend::new()));
    let shared_backend = backend.clone();

    // Start backend server, waiting for incoming connections from locos
    thread::spawn(move || backend_main(args.backend_port, backend));

    http_main(args.http_port, shared_backend).map_err(Error::HttpServer)?;

    Ok(())
}
