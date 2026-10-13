- **PERSON DECIDES THE DESIGN (2026-10-13)** The person approves each high-level
  connector design and each critical design decision, one decision per question,
  before code builds on it. `CLAUDE.md`, "The person decides the design", lists the
  kinds and the steps. No session makes one, and a record, code, a test, or a
  simulator that assumes a choice does not decide it. Cause: the influx kind wrote
  only to InfluxDB 1 (`POST /write?db=`). A simulator set that on 2026-10-07
  (977ae1606), each later PR built on it, and the ONE NODE interview asked it in one
  question with many other choices. The person, relayed by `laptop.monitor`
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6150069318,
  2026-10-13T16:17:11Z): "EVERY HIGH LEVEL CONNECTOR DESIGN AND CRITICAL DESIGN
  DECISION NEEDS TO BE RUN BY ME. PERIOD." Supersedes the list of four things that go
  to the person in `docs/coordination.md`
  (https://github.com/synnaxlabs/foundation/blob/e06be48ca/docs/coordination.md#L216-L222).
