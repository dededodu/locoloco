# LOCO-LOCO project

A repository gathering all tools for fully controlling model train through a Web browser interface.

## Loco Controller

### Usage

Run the controller as follows:
```
./loco_controller --http-port 8080 --backend-port 8004
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