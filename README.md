# PhaseRack

**Time-stretching and pitch-shifting** for the q-lib audio engine — a leaf DSP
crate in the audio split, built on the shared
[`sinerack`](https://github.com/QsKue/sinerack) core.

PhaseRack owns the `TimeStretcher` contract (change a signal's length and/or pitch,
independently) and its implementations. The [`mixrack`](https://github.com/QsKue/mixrack)
engine drives stretchers through this trait; PhaseRack carries no engine/session
knowledge of its own. Stretchers report their delay as a `sinerack::Latency` so the
engine can sum it across the pipeline.

## Implementations

- **`NoopTimeStretcher`** — pass-through; the default and a test baseline.
- **`WsolaTimeStretcher`** — dependency-free time-domain (Waveform Similarity Overlap-Add)
  **time-stretcher** (pitch-preserving speed change); available in all builds. Pitch shifting on
  this backend is a planned later increment.
- **`SignalSmithTimeStretcher`** — realtime stretcher/pitch-shifter backed by
  `signalsmith-stretch`, behind the `signalsmith` feature (on by default).

## License

Licensed under the GNU Affero General Public License v3.0 or later
([AGPL-3.0-or-later](LICENSE)).
