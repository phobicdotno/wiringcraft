#!/usr/bin/env sh
# Build a small motor-starter project from the command line and export it to PDF.
# Usage: examples/demo.sh [output-folder]   (run from the repository root)
# Set WIRINGCRAFT to the absolute path of a built wiringcraft binary to use it
# instead of `cargo run`.
set -eu
OUT=${1:-target/demo}
ROOT=$(pwd)
# Run the CLI: the WIRINGCRAFT binary when set (kept as one word, so a path
# with spaces works), else cargo run from the repository.
wcli() {
  if [ -n "${WIRINGCRAFT:-}" ]; then
    "$WIRINGCRAFT" "$@"
  else
    cargo run -q --manifest-path "$ROOT/Cargo.toml" -p wc-cli -- "$@"
  fi
}
LIB=$(pwd)/library
rm -rf "$OUT"
wcli new "$OUT" --name "Pump station P1"
cd "$OUT"
wcli lib add starter folder "$LIB"
wcli page add p0002 --number 2 --title "Main circuit"
wcli page add p0003 --number 3 --title "Control circuit"

# Page 2: breaker, main contacts, motor.
wcli place p0002 wc.breaker.1p --at 100,60 --tag "=PUMP+CAB1-Q1"
wcli place p0002 wc.contactor.main3 --at 140,110 --tag "=PUMP+CAB1-K1"
wcli place p0002 wc.motor.3ph --at 140,175 --tag "=PUMP+FIELD-M1"
wcli place p0002 wc.pe --at 190,200
wcli wire p0002 100,40 s1:1 --potential L1
wcli wire p0002 s1:2 s2:1 --via 100,90 --via 135,90 --number 1
wcli wire p0002 140,40 s2:3 --potential L2
wcli wire p0002 145,40 s2:5 --potential L3
wcli wire p0002 s2:2 s3:U --number 2
wcli wire p0002 s2:4 s3:V --number 3
wcli wire p0002 s2:6 s3:W --number 4
wcli wire p0002 s3:PE s4:PE --via 150,150 --via 190,150
# The 24 V control supply continues on page 3 through an interruption point.
wcli place p0002 wc.interruption --at 230,60 --tag=-24V
wcli wire p0002 230,40 s5:1 --potential 24V

# Page 3: start/stop with self-hold, coil, lamp.
wcli place p0003 wc.button.nc --at 60,60 --tag "=PUMP+CAB1-S1"
wcli place p0003 wc.button.no --at 60,100 --tag "=PUMP+CAB1-S2"
wcli place p0003 wc.contact.no --at 110,100 --tag "=PUMP+CAB1-K1"
wcli place p0003 wc.coil --at 60,150 --tag "=PUMP+CAB1-K1"
wcli place p0003 wc.contact.no --at 160,100 --tag "=PUMP+CAB1-K1"
wcli place p0003 wc.lamp --at 160,150 --tag "=PUMP+CAB1-P1"
wcli wire p0003 s1:12 s2:13 --number 10
wcli wire p0003 s1:12 s3:13 --via 60,85 --via 110,85
wcli wire p0003 s2:14 s4:A1 --number 11
wcli wire p0003 s3:14 s4:A1 --via 110,125 --via 60,125
wcli wire p0003 s4:A2 60,190 --potential 0V
wcli wire p0003 160,40 s5:13 --potential 24V
# The lamp sits in the field: it is wired through terminals -X1.1 and -X1.2.
wcli place p0003 wc.terminal --at 160,125 --tag "=PUMP+CAB1-X1.1"
wcli place p0003 wc.terminal --at 175,175 --tag "=PUMP+CAB1-X1.2"
wcli wire p0003 s5:14 s7:1 --number 12
wcli wire p0003 s7:2 s6:X1 --number 13 --cable=-W1 --core 1
wcli wire p0003 s6:X2 s8:1 --via 160,170 --via 175,170 --cable=-W1 --core 2
wcli wire p0003 s8:2 175,195 --potential 0V
# The 24 V supply arrives from page 2.
wcli place p0003 wc.interruption --at 60,40 --rotate 180 --tag=-24V
wcli wire p0003 s9:1 s1:11

# Parts from the example parts list in the starter library.
wcli device "=PUMP+CAB1-Q1" --part example.ex-mcb-1p-c10 --description "Pump supply"
wcli device "=PUMP+CAB1-K1" --part example.ex-ct-9-24dc --description "Pump contactor"
wcli device "=PUMP+CAB1-S1" --part example.ex-pb-red-nc --description "Stop"
wcli device "=PUMP+CAB1-S2" --part example.ex-pb-grn-no --description "Start"
wcli device "=PUMP+CAB1-P1" --part example.ex-lamp-24-gn --description "Pump running"
wcli device "=PUMP+CAB1-X1.1" --part example.ex-tb-2-5
wcli device --description "Lamp cable to the pump" -- -W1
wcli device "=PUMP+CAB1-X1.2" --part example.ex-tb-2-5

wcli lib lock
wcli vcs commit -m "Pump starter: main and control circuits"
wcli pdf pump-station.pdf --date 2026-11-16 --report all
wcli report parts
wcli report terminals
wcli report cables
wcli vcs log
