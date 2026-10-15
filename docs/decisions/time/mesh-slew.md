- **MESH SLEW (2026-10-05)** After the first estimate, mesh time moves toward each new
  estimate at no more than 500 ppm (ntpd's maximum slew), in `estimate::Slew`. The part
  not yet applied goes into the error, so a slew of 1 s takes 2000 s and its error says
  so. When the offset served at `now` is more than 500 us (1 s of slew) below the
  earliest offset the new estimate allows at `now`, mesh time steps forward to that
  earliest offset. In every other case it slews. Mesh time never steps back. A clock in
  holdover keeps its slew. Cost: mesh time that is ahead still slews. After a stale
  first estimate that is ahead, or for an estimate with an unknown error (a Windows OS
  clock alone under OS CLOCK BOUND, whose earliest offset is 36500 days back), a
  correction of 1 h takes 83 days and one of 1 day about 5.5 years, with a true error
  the whole time. A majority of falsetickers more than 500 us ahead steps mesh time into
  the future, and it does not come back. That is outside the fault model. Lost: a
  frequency loop (a PLL, as in ntpd), because R6 bounds drift with an error that grows
  and a PLL can overshoot; the slew private in `clock`, because it is decision logic in
  layer 2; a step only when the estimate's whole interval is ahead of mesh time's,
  because when both bounds hold the intervals overlap and it never fires. Amends R6 TIME
  LOCKED ("slew only") and r6 Q3 item 6 ("Step forward only at startup"). The person
  decided on 2026-10-05 ("Ok 225 mesh slew approved"), with the forward step. The person
  changed the forward step on 2026-10-05 ("I think (b)"), because the first rule never
  fires when both bounds hold.

  "Mesh time never steps back" holds for one run of a node. A node that starts takes its
  first mesh time from the first estimate of its sources, with no floor from a stored
  stamp or from the run before. So after the wall clock of the host steps back by more
  than the time that the node was down, mesh time starts before the stamps that the home
  stored, and the home refuses each live stamp that is not after the last one of its
  index (`Backwards`, A5) until mesh time passes it: about 1 h after a step back of 1 h.
  A connector writes its status frame again after that stamp (SUPERVISOR), and the home
  takes it only within the ahead limit (SHARD HOMES), so after a larger step the status
  waits, and with it the next change of state of the connector. Cost: the node takes no
  data on those indexes for that time, and nothing tells the operator the cause but the
  refusal that a writer gets. Lost: a floor at the newest stored stamp, because the slew
  then takes 2000 times the step to remove it (83 days for 1 h), because a restart is
  the one way to end a slew of mesh time that is ahead, and because a writer picks its
  stamps up to the ahead limit after mesh time, so one writer with a fast clock moves
  mesh time ahead at each restart; that floor less the ahead limit, because the first
  two causes hold for it; a floor from a file that `clock` writes, because the first two
  causes hold for it and it adds a durable write to the clock; a stamp after the last
  stored one for each refused frame, because the home refuses it as `Ahead` after a step
  of more than the ahead limit. By `laptop.architect-2` (#2406, 2026-10-11 15:43 UTC:
  https://github.com/synnaxlabs/foundation/issues/2406#issuecomment-6110730243). The
  person chose it over the floor: "(a) is fine for now" (2026-10-12 15:38 UTC, relayed
  by `laptop.monitor`:
  https://github.com/synnaxlabs/foundation/issues/2406#issuecomment-6127519775).
