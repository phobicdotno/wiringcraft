# Manufacturer schematic data: who publishes what

Research for issue #46 (November 2026). Where component makers publish symbols, macros, part
data and 2D/3D drawings for electrical CAD, in which formats, whether a login is needed, under
which terms, and which of it WiringCraft can read today.

Nothing listed here is downloaded into this repository. Each site's terms apply to whoever
downloads the data. "Not stated" means the page we checked did not say; check the site's own
terms before relying on a download.

## What WiringCraft reads today

From `crates/wc-import`:

| Format | Module | Notes |
|---|---|---|
| ASCII DXF | `dxf` | Symbols and whole drawings. Binary DXF and DWG are **not** read. |
| KiCad symbol libraries (`.kicad_sym`, KiCad 6+) | `kicad` | Graphics and pins of the common unit and the first unit (body style 1) become a WiringCraft symbol; later units and alternate body styles are skipped with a warning. |
| SVG | `svg` | Graphics. |
| PDF (vector) | `pdf` | Traced drawings. |
| CSV | `csv` | Parts lists and PLC address lists. |
| AutomationML (IEC 62714, CAEX XML) | `aml` | Devices, properties, connection points, links. |
| EPLAN P8 projects (`.elk` + `.edb`) | `eplan` | Projects, not the EDZ macro/part exchange files. |
| SolidWorks Electrical archives (`.tewzip`) | `swe` | Projects. Content Portal parts also arrive as `.tewzip`; library import is #62. |
| Plain zip | `archive` | Opens the container and imports PDF, DXF, AutomationML and CSV entries inside. Other entries (including `.kicad_sym`) are listed, not imported. |

Not read today: **EDZ** (EPLAN Data Exchange archives with macros and parts), **DWG**, **STEP**
(3D), **BMEcat** and **ECLASS** part data, QElectroTech **`.elmt`** elements.

## Portals that serve many manufacturers

| Portal | What | Formats | Login / cost | Terms | WiringCraft today |
|---|---|---|---|---|---|
| [EPLAN Data Portal](https://www.eplan.com/us-en/products/eplan-data-portal) | 550+ manufacturers, 2.3 million+ components: commercial and technical data, schematic macros, 3D, images | EDZ / EPLAN macros | Free **with a current EPLAN licence subscription and an EPLAN ID**. Viewing without a licence has been allowed since Data Portal 2.1 ([news](https://www.automation.com/article/eplan-updates-data-portal-to-allow-free-viewing)). | Made for use inside EPLAN ([portal terms](https://www.eplan.com.cn/content/dam/eplan/corporate/legal/terms-of-use/eplan-epulse-portal-terms-of-use-10-2020-cn-cn.pdf)) | No (EDZ) |
| [SOLIDWORKS Electrical Content Portal](https://help.solidworks.com/2025/English/SWElec/r_swelec_UsingECP.htm) | Manufacturer parts, cable references, symbols. TE Connectivity via TraceParts ([blog](https://blogs.solidworks.com/products/solidworks/traceparts-collaboration-integrating-the-te-connectivity-catalog-into-the-electrical-content-portal-ecp)) | `.tewzip` | 3DEXPERIENCE ID plus an active SOLIDWORKS Electrical Schematic subscription ([Javelin](https://www.javelin-tech.com/blog/2025/02/using-the-solidworks-electrical-content-portal/)) | Dassault terms | Container yes; parts library is #62 |
| CADENAS PARTcommunity / TraceParts | 2D/3D CAD for thousands of makers, some ECAD macros | 80+ native and neutral formats incl. DXF, DWG, STEP | Free account | Per portal and per maker | DXF yes (ASCII) |
| [KiCad libraries](https://www.kicad.org/libraries/license/index.html) | Generic and manufacturer-specific symbols (mostly electronics) | `.kicad_sym` | Free, no login (GitLab) | CC-BY-SA 4.0 with the KiCad libraries exception: free to use in any design, closed or commercial; redistributing the libraries as a collection keeps CC-BY-SA | **Yes** |
| [QElectroTech element collection](https://qelectrotech.org/) and [contrib repo](https://github.com/qelectrotech/qelectrotech-element-contrib) | IEC schematic elements, manufacturer variants (Hager, ABB, Hensel, Shelly, WAGO planned) | `.elmt` (XML) | Free, no login | Contrib repo under GPL-2.0; check the main collection's licence per element | No |

## Manufacturers

| Manufacturer | Product area | Where | Formats | Login | Terms | WiringCraft today |
|---|---|---|---|---|---|---|
| **WAGO** | Terminal blocks, I/O, interface | [Product pages, Downloads / CAE data](https://www.wago.com/global/digitization/digital-engineering/eplan); EPLAN Data Portal; [other CAE tools](https://www.wago.com/global/digitization/digital-engineering/engineering-data) | EPLAN Data Standard macros (11,000+ products, logic macros, 3D); WSCAD, StriePlan, AutoCAD Electrical, SEE Electrical, HagerCAD; 2D/3D via CADENAS | Product pages: not stated; Data Portal: EPLAN ID | Not stated | 2D DXF via CADENAS yes; EPLAN no. No SolidWorks Electrical catalogue confirmed. |
| **Phoenix Contact** | Terminals, connectors, power, I/O | [Product data for engineering](https://phoenixcontact.com/en-gb/product-data-for-your-engineering-process) | EPLAN P8 file generator and EPLAN Data Portal; AutoCAD Electrical, SEE Electrical, WSCAD, Zuken, Aucotec, AmpereSoft, SDProget, NX Electrical; BMEcat XML; ECLASS | Not stated | Not stated | No direct path; BMEcat is a candidate |
| **Siemens** | PLCs, drives, switchgear | Industry Support product page, **CAx data** tab, CAx Download Manager ([how-to](https://industrialmonitordirect.com/blogs/knowledgebase/download-siemens-eplan-edz-macros-via-cax-data-manager)) | EPLAN P8 macros (EDZ); 2D/3D CAD (DXF, STEP) without connection points | Not stated; the download tool needs Java | Siemens terms | DXF yes (graphics only) |
| **Schneider Electric** | Switchgear, drives, PLCs | se.com document downloads (e.g. [EPLAN EDZ](https://www.se.com/hk/en/download/document/EPLAN_EDZ_Spacial_SM/)); EPLAN Data Portal | EDZ, EPLAN macro templates; DXF/DWG per product | Not stated | Schneider terms | DXF yes |
| **Eaton** | Switchgear, motor control | [ECAD / MCAD / EPLAN tools](https://www.eaton.com/us/en-us/support/tools/ecad-mcad-eplan-.html): eCAD Data Portal and mCAD Data Portal on PARTcommunity; EPLAN Data Portal (30,000+ products) | eCAD macros in native formats plus neutral DXF/DWG; 2D/3D in 80+ formats | Personal eCAD/PARTcommunity account | Portal terms | DXF yes |
| **ABB** | Low voltage products, drives, PLCs | [ABB EPLAN page](https://new.abb.com/low-voltage/de/niederspannungsprodukte/eplan); EPLAN Data Portal | EPLAN macros and parts | EPLAN Data Portal: EPLAN ID | Not stated | No |
| **Mitsubishi Electric FA** | PLCs, servo, inverters | [FA download centre, CAD data](https://www.mitsubishielectric.com/fa/download/); EPLAN Data Portal | 2D DWG (2000) and DXF; 3D IGES, Parasolid, STEP AP203 | Accept terms; login not stated | [CAD terms](https://www.mitsubishielectric.com/fa/products/cnt/plc_fx/dldb/copyright_cad.html): for design consideration, simplified geometry, no guarantee of specs | DXF yes (graphics only) |
| **Rockwell Automation / Allen-Bradley** | PLCs, drives, motor control | [Product compatibility and download centre](https://www.rockwellautomation.com/en-us/support/product) | EPLAN and AutoCAD Electrical drawings, DXF/DWG, 3D | Not checked | Rockwell terms | DXF yes |
| **SEW-EURODRIVE** | Drives, gear motors | [CAE data in Online Support](https://download.sew-eurodrive.com/download/html/27806162/en-EN/41660761483.html); EPLAN Data Portal | EDZ | Data Portal: register and log in | Not stated | No |
| **Dold** | Safety and monitoring relays | [EPLAN macros](https://dold.com/en/service/eplan) on product pages and the EPLAN Data Portal | EPLAN product macros | Not stated | Business customers only for sales | No |
| **Euchner** | Safety switches | [EPLAN macros](https://www.euchner.com/en-us/service/downloads/eplan-macros/), Downloads tab, EPLAN Data Portal | EPLAN macros | Not stated | Not stated | No |
| **Lenze** | Inverters, motor starters | [E-catalogue per product](https://www.lenze.com/bg-bg/go/akb/200400010/1) | EPLAN macros (schematic and mounting plate), article data | Not stated; described as free | Not stated | No |
| **igus** | Pre-assembled cables (readycable) | EPLAN Data Portal, linked from the online shop ([press](https://press.igus.eu/igus-is-adding-2046-new-macros-to-the-eplan-circuit-diagram-software/)) | EPLAN macros (2,046 cable macros and more) | EPLAN ID | Not stated | No |
| **Beckhoff** | I/O, IPCs, drives | Beckhoff download site ([install guide](https://download.beckhoff.com/download/Document/fi/Install_EPLAN_Macros.pdf)) | EPLAN macros | Not stated | Not stated | No |
| **Festo** | Pneumatics, valve terminals | EPLAN Data Portal ([news](https://news.thomasnet.com/companystory/2-000-festo-devices-macros-developed-for-eplan-users-20000086)) | EPLAN macros | EPLAN ID | Not stated | No |
| **KEB** | Drives | [KEB macros in the EPLAN Data Portal](https://www.keb-automation.com/technology/industrial-standards/topics/eplan) | EPLAN macros | EPLAN ID | Not stated | No |
| **Rittal** | Enclosures, climate | [Engineering](https://www.rittal.com/au-en/Solutions/Enclosure-manufacturing-40/Engineering); EPLAN Data Portal | EPLAN macros, 3D | EPLAN ID | Not stated | No |
| **Weidmüller** | Terminals, connectors | [Digital engineering downloads](https://www.weidmuller.com/us/downloads/gsd---eds-files) | EPLAN and other CAE data | Not stated | Not stated | No |

## What this means for WiringCraft

1. **Readable now:** KiCad libraries (open licence with clear terms, read directly), and any
   maker's 2D **ASCII DXF** drawings for symbol graphics. Being able to read a file is not
   permission to use it: most makers' terms are not stated on the pages checked, and some limit
   use (Mitsubishi's CAD data is "for design consideration" only), so check each maker's terms
   before building a library from their files. DXF from Siemens and similar sites has no
   connection points, so pins must be added after import.
2. **The biggest pool is EPLAN EDZ.** Almost every industrial maker publishes EPLAN macros, most
   through the EPLAN Data Portal, which needs an EPLAN licence. Many also put EDZ files on their
   own product pages without the portal (Siemens CAx data, Schneider, SEW, Dold, Euchner, Lenze,
   Beckhoff). An EDZ importer (macros to symbols, parts to the library) would open that pool for
   people who download from those pages; it should be built from the public format and own test
   files, as the EPLAN project importer was.
3. **Part data without drawings:** Phoenix Contact's BMEcat XML and ECLASS classification carry
   order numbers, descriptions and technical data. A BMEcat import would fill the parts library
   for makers who publish it.
4. **QElectroTech `.elmt`** is an open XML format with a free collection that includes some
   manufacturer variants (Hager, ABB, Shelly). An importer would add many IEC symbols with
   connection points.
5. **DWG** is common (Eaton, Mitsubishi, Rockwell) but closed; DXF is usually offered next to it.
6. **SolidWorks Electrical Content Portal** data needs a SolidWorks subscription; its `.tewzip`
   parts are covered by the library import in #62.

Follow-up candidates for the todo list: EDZ import, BMEcat parts import, QElectroTech `.elmt`
symbol import.
