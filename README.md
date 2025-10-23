# LOCO-LOCO project

A repository gathering all tools for fully controlling model train through a Web browser interface.

## Pre-requisites

Both `rustup` and `cargo` should be installed on your machine.
Also, install `picotool` following the instructions from
https://github.com/raspberrypi/picotool

### Rustup targets

```
rustup target add aarch64-unknown-linux-gnu
rustup target add thumbv8m.main-none-eabihf
```

### Udev rules

Copy udev rules over to your machine in order to allow for running `picotool`
without the need for `sudo` privileges, and also to make the board show up
as `/dev/ttyACM*` while running. This allows for easier debugging.

```
sudo cp udev_rules/99-rp-pico2w.rules /etc/udev/rules.d/
```

## Loco Controller

### Build

```
cargo build --target aarch64-unknown-linux-gnu
```

### Usage

Run the controller as follows:
```
./loco_controller \
    --http-port 8080 \
    --backend-locos-port 8004 \
    --backend-sensors-port 8005 \
    --backend-actuators-port 8006 \
```

### HTTP requests

Use `cURL` for sending requests to the HTTP server.

#### Check server is running

```
curl -X GET http://localhost:8080/
```

#### Query status of a loco

```
curl -X GET http://localhost:8080/loco_status/loco1
```

#### Control a loco

```
curl -X POST http://localhost:8080/control_loco \
    -H 'Content-Type: application/json' \
    -d '{"loco_id":"loco1", "direction": "forward", "speed": "fast"}'
```

#### Drive a switch rails

```
curl -X POST http://localhost:8080/drive_switch_rails \
    -H 'Content-Type: application/json' \
    -d '{"actuator_id":"switchrails1", "state": "direct"}'
```

## Pico programs

### Loco Pico

This is the code running on the Pi Pico 2 W embedded in every loco. It registers
itself to the `loco_controller` so that later on it can receive some requests
coming from the `loco_controller`. A request can be the result of an HTTP
request being forwarded all the way to the loco, or simply due to some internal
requirements.

### Sensors Pico

This is the code running on the Pi Pico 2 W attached to all RFID readers. These
readers act as waypoints to locate locos and report these information to the
`loco_controller`. The program connects to the `loco_controller` and reports
periodically the updated locations if any. Then, it's up to the
`loco_controller` to decide what to do with these information. The location is
reported through the HTTP request `loco_status`.

### Build

```
cargo build --target thumbv8m.main-none-eabihf
```

### Flash the board

__loco_pico__
```
picotool load -t elf target/thumbv8m.main-none-eabihf/debug/loco_pico -fx
```

__sensors_pico__
```
picotool load -t elf target/thumbv8m.main-none-eabihf/debug/sensors_pico -fx
```

__actuators_pico__
```
picotool load -t elf target/thumbv8m.main-none-eabihf/debug/actuators_pico -fx
```

### Debug logs

Display logs from the Pi Pico 2 W board by connecting it to USB on your machine
and by running `screen` command:

```
screen /dev/ttyACM0
```
