# warble

**Time-stretching and pitch-shifting** for the q-lib audio engine — a leaf DSP
crate in the audio split, built on the shared
[`prism`](https://github.com/QsKue/prism) core.

warble owns the `TimeStretcher` contract (change a signal's length and/or pitch,
independently) and its implementations. The [`maestro`](https://github.com/QsKue/maestro)
engine drives stretchers through this trait; warble carries no engine/session
knowledge of its own. Stretchers report their delay as a `prism::Latency` so the
engine can sum it across the pipeline.

## Implementations

- **`NoopTimeStretcher`** — pass-through; the default and a test baseline.
- **`SignalSmithTimeStretcher`** — realtime stretcher/pitch-shifter backed by
  `signalsmith-stretch`, behind the `signalsmith` feature (on by default).

## License

Licensed under the GNU Affero General Public License v3.0 or later
([AGPL-3.0-or-later](LICENSE)).
