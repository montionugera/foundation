# Quickstart: OPC UA to InfluxDB

This page starts one Foundation node that reads three values from an OPC UA server and
pushes each sample to InfluxDB.

## What you need

- Rust 1.98 or later, and a C compiler.
- An OPC UA server with security policy None, and InfluxDB 1.8 or a later 1.x release
  with no authentication. To use your own, change the addresses and the node ids in
  step 2. Else run these two in Docker:

```sh
docker run -d --name opcua -p 50000:50000 \
  mcr.microsoft.com/iotedge/opc-plc --pn=50000 --ut --aa
docker run -d --name influxdb -p 8086:8086 -e INFLUXDB_DB=plant influxdb:1.8
```

## 1. Install

```sh
cargo install --locked --git https://github.com/synnaxlabs/foundation node
```

This builds the `foundation` binary into `~/.cargo/bin`. Check that your shell finds
it:

```sh
foundation version
```

## 2. Write the config

Make an empty directory, and save this file in it as `plant.hcl`:

```hcl
# The time of each sample from the OPC UA server.
channel "plant.time" { kind = "index" }

# A value that spikes now and then.
channel "plant.spike" {
  data_type = "f64"
  index     = "plant.time"
}

# A value that dips now and then.
channel "plant.dip" {
  data_type = "f64"
  index     = "plant.time"
}

# A value that rises.
channel "plant.trend" {
  data_type = "f64"
  index     = "plant.time"
}

# On node "edge", reads three values from the OPC UA server 10 times a second.
connector "plc" {
  kind    = "opcua"
  node    = "edge"
  address = "opc.tcp://localhost:50000"
  rate    = "10Hz"
  read "plant.spike" { node_id = "ns=3;s=SpikeData" }
  read "plant.dip" { node_id = "ns=3;s=DipData" }
  read "plant.trend" { node_id = "ns=3;s=PositiveTrendData" }
}

# On node "edge", pushes each sample to InfluxDB.
connector "influx" {
  kind        = "influx"
  node        = "edge"
  address     = "http://localhost:8086"
  database    = "plant"
  measurement = "plant"
  select      = "plant.*"
}
```

A `channel` block makes a channel. An index channel holds the time of each sample,
and each data channel points at its index. A `connector` block runs on the node that
`node` names. The `rate` of the `opcua` connector sets how often it reads.

## 3. Start the node

In the same directory:

```sh
foundation start --name edge
```

The node runs until you stop it with Ctrl-C. It keeps its data in `foundation-data`,
and the first start gives it the name `edge`. It prints:

```text
node edge runs in foundation-data. Stop it with Ctrl-C.
```

## 4. Plan and apply the config

In a second terminal, in the same directory, see what the config changes:

```sh
foundation plan plant.hcl --out plant.plan
```

```text
+ channel plant.time
+ channel plant.spike
+ channel plant.dip
+ channel plant.trend
+ connector plc
+ channel plc.status.backoff
+ channel plc.status.class
+ channel plc.status.error
+ channel plc.status.in
+ channel plc.status.lost
+ channel plc.status.missed
+ channel plc.status.restarts
+ channel plc.status.state
+ channel plc.status.time
+ connector influx
+ channel influx.status.backoff
+ channel influx.status.class
+ channel influx.status.confirmed
+ channel influx.status.error
+ channel influx.status.restarts
+ channel influx.status.state
+ channel influx.status.time
22 to add, 0 to change, 0 to remove.
note: 2 definitions that Foundation made are not in the files: @admin.@access, @admin.@subject
```

A connector implies the status channels under its name, such as `plc.status.state`,
and a channel for each count of its kind, such as `plc.status.in`. The note names the
definitions that the first start made, which no file holds.

Apply exactly that plan:

```sh
foundation apply plant.plan
```

```text
Applied plant.plan: 22 added, 3 homes listed.
```

`apply` refuses a plan when the node's config changed after the plan. Plan again.

## 5. See samples flow

```sh
foundation status
```

```text
CONNECTOR  KIND    STATE    ADDRESS                    IN    CONFIRMED
influx     influx  running  http://localhost:8086      -     1517
plc        opcua   running  opc.tcp://localhost:50000  1520  -
```

`IN` counts the samples that the connector read from the server and the node stored.
`CONFIRMED` counts the samples that InfluxDB confirmed. Run `status` again, and both
grow.

See the samples in InfluxDB:

```sh
docker exec influxdb influx -database plant -execute 'SELECT * FROM plant LIMIT 5'
```

## When something is wrong

A config with an error gives the file, line, column, and a fix, and `plan` writes no
plan. This is the output when `data_type` of `plant.spike` is written as `datatype`:

```text
error[document.missing-attribute]: the `channel` block has no `data_type`
  --> plant.hcl:5:1
fix: Add a `data_type` attribute such as "f64"

error[document.unknown-attribute]: `datatype` is not an attribute of the `channel` block
  --> plant.hcl:6:3
fix: Use `kind`, `data_type`, `index`, `quality`, or `unit`, or remove it
```

A connector that cannot reach its address shows the cause in `status`. Fix the address
in `plant.hcl`, then plan and apply again.

## Clean up

Stop the node with Ctrl-C. Then:

```sh
docker rm -f opcua influxdb
rm -r foundation-data plant.plan
```
