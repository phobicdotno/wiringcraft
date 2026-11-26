#!/usr/bin/env python3
"""Generate WiringCraft's starter symbol library (library/symbols/*.json).

All symbols are original drawings in the general style of IEC 60617 schematic symbols,
on a 5 mm grid, origin at the top pin, y pointing down. Run from the repository root.
"""
import json, os

OUT = os.path.join("library", "symbols")

def P(x, y): return {"x": float(x), "y": float(y)}
def line(a, b): return {"kind": "line", "a": P(*a), "b": P(*b)}
def rect(a, b): return {"kind": "rect", "min": P(*a), "max": P(*b)}
def circle(c, r, filled=False):
    d = {"kind": "circle", "center": P(*c), "radius": float(r)}
    if filled: d["filled"] = True
    return d
def polyline(pts, filled=False):
    d = {"kind": "polyline", "points": [P(*p) for p in pts], "closed": True}
    if filled: d["filled"] = True
    return d
def arc(c, r, s, e): return {"kind": "arc", "center": P(*c), "radius": float(r), "start": float(s), "end": float(e)}
def text(at, t, h=2.5, align=None):
    d = {"kind": "text", "at": P(*at), "text": t, "height": float(h)}
    if align: d["align"] = align
    return d
def pin(name, x, y, d): return {"name": name, "at": P(x, y), "dir": d}

def vpins(top, bottom, length=15):
    return [pin(top, 0, 0, "up"), pin(bottom, 0, length, "down")]

def no_contact():
    # Leads, then a blade swung open to the left.
    return [line((0, 0), (0, 5)), line((0, 10), (0, 15)), line((0, 10), (-3.5, 4.2))]

def nc_contact():
    return [line((0, 0), (0, 5)), line((0, 5), (2.5, 5)), line((0, 10), (0, 15)), line((0, 10), (3, 4.2))]

def pin_label(p):
    x, y = p["at"]["x"], p["at"]["y"]
    if p["dir"] == "right": return text((x - 1.4, y - 1.4), p["name"], 2.0)
    if p["dir"] == "left": return text((x + 0.4, y - 1.4), p["name"], 2.0)
    return text((1.2, y + (2.5 if p["dir"] == "up" else -0.6)), p["name"], 2.0)

def pin_labels(pins):
    return [pin_label(p) for p in pins]

S = []
def sym(id, name, cat, code, role, graphics, pins, tag_at=(-6, 8), desc=""):
    s = {"id": id, "name": name, "category": cat, "letter_code": code, "role": role,
         "graphics": graphics + pin_labels(pins), "pins": pins, "tag_at": P(*tag_at)}
    if desc: s["description"] = desc
    S.append(s)

sym("wc.contact.no", "Normally open contact", "contacts", "K", "auxiliary", no_contact(), vpins("13", "14"),
    desc="Auxiliary make contact, driven by the coil with the same tag.")
sym("wc.contact.nc", "Normally closed contact", "contacts", "K", "auxiliary", nc_contact(), vpins("11", "12"),
    desc="Auxiliary break contact, driven by the coil with the same tag.")
sym("wc.coil", "Relay coil", "relays", "K", "main",
    [line((0, 0), (0, 4)), rect((-4, 4), (4, 11)), line((0, 11), (0, 15))], vpins("A1", "A2"),
    tag_at=(-12, 8), desc="Coil of a relay or contactor; its contacts carry the same tag.")
sym("wc.fuse", "Fuse", "protection", "F", "standalone",
    [line((0, 0), (0, 15)), rect((-1.8, 3.5), (1.8, 11.5))], vpins("1", "2"))
sym("wc.breaker.1p", "Circuit breaker, 1-pole", "protection", "Q", "standalone",
    no_contact() + [line((-1, 4), (1, 6)), line((1, 4), (-1, 6)), rect((-6.5, 6), (-4.5, 8))], vpins("1", "2"))
sym("wc.button.no", "Push button, make", "operators", "S", "standalone",
    no_contact() + [line((-1.8, 7), (-6, 7)), line((-6, 5.5), (-6, 8.5))], vpins("13", "14"), tag_at=(-12, 8))
sym("wc.button.nc", "Push button, break", "operators", "S", "standalone",
    nc_contact() + [line((1.5, 7), (-4, 7)), line((-4, 5.5), (-4, 8.5))], vpins("11", "12"), tag_at=(-12, 8))
sym("wc.lamp", "Indicator lamp", "signalling", "P", "standalone",
    [line((0, 0), (0, 3.5)), circle((0, 7.5), 4), line((-2.8, 4.7), (2.8, 10.3)), line((2.8, 4.7), (-2.8, 10.3)),
     line((0, 11.5), (0, 15))], vpins("X1", "X2"), tag_at=(-12, 8))
sym("wc.terminal", "Terminal", "terminals", "X", "terminal",
    [line((0, 0), (0, 5.5)), circle((0, 7.5), 2), line((0, 9.5), (0, 15))], vpins("1", "2"), tag_at=(-8, 8),
    desc="One terminal of a terminal strip; the strip is the device tag, the terminal number is its index.")
sym("wc.motor.3ph", "Three-phase motor", "drives", "M", "standalone",
    [line((0, 0), (0, 7)), line((-5, 0), (-5, 5), ), line((-5, 5), (-3.5, 7.3)), line((5, 0), (5, 5)), line((5, 5), (3.5, 7.3)),
     circle((0, 13), 6.5), text((0, 13), "M", 4, "center"), text((0, 17.2), "3~", 2.5, "center"),
     line((10, 0), (10, 13)), line((10, 13), (6.5, 13))],
    [pin("U", -5, 0, "up"), pin("V", 0, 0, "up"), pin("W", 5, 0, "up"), pin("PE", 10, 0, "up")], tag_at=(-16, 13))
sym("wc.contactor.main3", "Main contacts, 3-pole", "contacts", "Q", "auxiliary",
    [line((-5, 0), (-5, 5)), line((-5, 10), (-5, 15)), line((-5, 10), (-8.5, 4.2)), arc((-5, 5), 1.0, 180, 360),
     line((0, 0), (0, 5)), line((0, 10), (0, 15)), line((0, 10), (-3.5, 4.2)), arc((0, 5), 1.0, 180, 360),
     line((5, 0), (5, 5)), line((5, 10), (5, 15)), line((5, 10), (1.5, 4.2)), arc((5, 5), 1.0, 180, 360),
     line((-6.8, 7.2), (3.2, 7.2))],
    [pin("1", -5, 0, "up"), pin("2", -5, 15, "down"), pin("3", 0, 0, "up"), pin("4", 0, 15, "down"),
     pin("5", 5, 0, "up"), pin("6", 5, 15, "down")], tag_at=(-16, 8))
sym("wc.psu.dc", "DC power supply", "power", "G", "standalone",
    [rect((-10, 5), (10, 20)), line((-10, 20), (10, 5)), text((-6, 9), "~", 3), text((4, 18.5), "=", 3),
     line((-5, 0), (-5, 5)), line((5, 0), (5, 5)), line((-5, 20), (-5, 25)), line((5, 20), (5, 25))],
    [pin("L", -5, 0, "up"), pin("N", 5, 0, "up"), pin("+", -5, 25, "down"), pin("-", 5, 25, "down")], tag_at=(-20, 12))
sym("wc.plc.input", "PLC digital input", "plc", "A", "standalone",
    [line((0, 0), (0, 5)), rect((-7, 5), (7, 12)), text((0, 9.8), "DI", 2.5, "center")], [pin("I", 0, 0, "up")], tag_at=(-16, 9))
sym("wc.plc.output", "PLC digital output", "plc", "A", "standalone",
    [rect((-7, 0), (7, 7)), text((0, 4.8), "DO", 2.5, "center"), line((0, 7), (0, 12))], [pin("Q", 0, 12, "down")], tag_at=(-16, 4))
sym("wc.pe", "Protective earth", "power", "", "standalone",
    [line((0, 0), (0, 5)), line((-4, 5), (4, 5)), line((-2.6, 6.5), (2.6, 6.5)), line((-1.2, 8), (1.2, 8))],
    [pin("PE", 0, 0, "up")], tag_at=(4, 3))
sym("wc.fe", "Functional earth", "power", "", "standalone",
    [line((0, 0), (0, 5)), line((-4, 5), (4, 5)), line((-2.6, 6.5), (2.6, 6.5)), line((-1.2, 8), (1.2, 8))],
    [pin("FE", 0, 0, "up")], tag_at=(4, 3),
    desc="Functional (signal) earth: 0 V references, signal grounds and screen earthing. Not a protective earth; use wc.pe for the protective conductor.")
sym("wc.potentiometer", "Potentiometer", "passive", "R", "standalone",
    # Resistor body with the wiper arrow touching its right side.
    [line((0, 0), (0, 6.5)), rect((-1.8, 6.5), (1.8, 13.5)), line((0, 13.5), (0, 20)),
     line((1.8, 10), (5, 10)), line((1.8, 10), (3.3, 9.1)), line((1.8, 10), (3.3, 10.9))],
    [pin("1", 0, 0, "up"), pin("2", 5, 10, "right"), pin("3", 0, 20, "down")], tag_at=(-6, 10),
    desc="Adjustable resistor (setpoint, speed reference); 1 and 3 are the ends, 2 is the wiper.")
sym("wc.connector.socket", "Socket (female contact)", "terminals", "X", "standalone",
    # Lead into a half-circle cup that opens downwards, towards the mating plug.
    [line((0, 0), (0, 5)), arc((0, 7.5), 2.5, 180, 360)],
    [pin("1", 0, 0, "up")], tag_at=(-6, 5),
    desc="One contact of a plug-in connector, female side; the mating side is open at the bottom.")
sym("wc.connector.plug", "Plug (male contact)", "terminals", "X", "standalone",
    # Solid pin end at the top, pointing at the socket it mates with, lead down to pin 1.
    [polyline([(-0.8, 0), (0.8, 0), (0.8, 4), (-0.8, 4)], filled=True), line((0, 4), (0, 10))],
    [pin("1", 0, 10, "down")], tag_at=(-6, 5),
    desc="One contact of a plug-in connector, male side; the solid pin end at the top mates with a socket.")

os.makedirs(OUT, exist_ok=True)
for s in S:
    with open(os.path.join(OUT, s["id"] + ".json"), "w", newline="\n") as f:
        f.write(json.dumps(s, indent=2, sort_keys=True) + "\n")
print(f"wrote {len(S)} symbols to {OUT}")
