---
name: masterformat-process
description: "MasterFormat Level 2 codes for vendor groups. Covers process equipment (divisions 40-48): process interconnections, material processing/handling, process heating/cooling/drying, pollution/waste control, industry-specific manufacturing, and electrical power generation."
---

# MasterFormat Level 2 Reference — Process Equipment

Covers process equipment (divisions 40-48): process interconnections, material processing/handling, process heating/cooling/drying, pollution/waste control, industry-specific manufacturing, and electrical power generation.

## How to Use MasterFormat Skills

**Always attempt a MasterFormat code lookup for every vendor group**, regardless of document format.

1. **Choose the right skill** based on work scope:
   - Building or facility specification (divisions 02-28) -> **masterformat-building**
   - Civil, infrastructure, or site work (divisions 31-35) -> **masterformat-civil**
   - Process or industrial equipment (divisions 40-48) -> this skill (**masterformat-process**)
   - Mixed scope -> read whichever skill best fits the majority, or read multiple skills

2. **If the document IS CSI MasterFormat** (section numbers like `40 10 00`, `44 20 00`, or "Division XX" headings):
   - Use the document's section codes directly as `masterformat_code`
   - Validate each code exists in the skill table. If a code is not in the table, set `masterformat_code: null`

3. **If the document is NOT CSI MasterFormat** (NatSpec, NBS, work packages, proprietary scope breakdowns, trade lists, etc.):
   - Look up the scope in the MasterFormat skill table and find the closest matching code
   - Example: a "Water Treatment" scope package -> look up water treatment in this skill -> assign `"44 40 00"` (or the best-matching code)
   - If truly no code in the table matches the vendor group's scope, set `masterformat_code: null`

4. **If uncertain** about the best match -> set `masterformat_code: null`. Never force a match.

## Reference Table

Use this table to assign `masterformat_code` to each vendor group.
Pick the code whose title best matches the scope of work.
If no code fits, set `masterformat_code: null`.

| Code     | Title                                        |
|----------|----------------------------------------------|
| 40 01 00 | Operation and Maintenance of Process Interconnections |
| 40 05 00 | Common Work Results for Process Interconnections |
| 40 06 00 | Schedules for Process Interconnections |
| 40 10 00 | Gas and Vapor Process Piping and Ductwork |
| 40 11 00 | Steam Process Piping |
| 40 12 00 | Compressed Air Process Piping |
| 40 13 00 | Process Ductwork |
| 40 15 00 | Combustion System Gas Piping |
| 40 16 00 | Specialty and High-Purity Gases Piping |
| 40 17 00 | Welding and Cutting Gases Piping |
| 40 18 00 | Vacuum Systems Process Piping |
| 40 20 00 | Liquids Process Piping |
| 40 21 00 | Liquid Fuel Process Piping |
| 40 22 00 | Petroleum Products Piping |
| 40 23 00 | Water and Wastewater Process Piping |
| 40 24 00 | Specialty Liquid Chemicals Piping |
| 40 25 00 | Liquid Acids and Bases Piping |
| 40 26 00 | Liquid Polymer Piping |
| 40 30 00 | Solid and Mixed Materials Piping and Chutes |
| 40 32 00 | Bulk Materials Piping and Chutes |
| 40 33 00 | Bulk Materials Valves |
| 40 34 00 | Pneumatic Conveying Lines |
| 40 40 00 | Process Piping and Equipment Protection |
| 40 41 00 | Process Piping and Equipment Heat Tracing |
| 40 42 00 | Process Piping and Equipment Insulation |
| 40 46 00 | Process Corrosion Protection |
| 40 47 00 | Refractories |
| 40 60 00 | Process Control and Enterprise Management Systems |
| 40 61 00 | Process Control and Enterprise Management Systems General Provisions |
| 40 62 00 | Computer System Hardware and Ancillaries |
| 40 63 00 | Control System Equipment |
| 40 66 00 | Network and Communication Equipment |
| 40 67 00 | Control System Equipment Panels and Racks |
| 40 68 00 | Process Control Software |
| 40 69 00 | Packaged Control Systems |
| 40 70 00 | Instrumentation for Process Systems |
| 40 71 00 | Flow Measurement |
| 40 72 00 | Level Measurement |
| 40 73 00 | Pressure, Strain, and Force Measurement |
| 40 74 00 | Temperature Measurement |
| 40 75 00 | Process Liquid Analytical Measurement |
| 40 76 00 | Process Gas Analytical Measurement |
| 40 77 00 | Position and Motion Measurement |
| 40 78 00 | Panel Mounted Instruments |
| 40 79 00 | Miscellaneous Instruments, Calibration Equipment, Instrument Valves, and Fittings |
| 40 80 00 | Commissioning of Process Systems |
| 40 90 00 | Primary Control Devices |
| 40 91 00 | Primary Control Valves |
| 40 92 00 | Self-Contained Flow Controllers |
| 40 93 00 | Linear Actuators and Positioners |
| 40 94 00 | Self-Contained Pressure Regulators |
| 40 95 00 | Rotary Actuators |
| 40 96 00 | Saturable Core Reactors |
| 40 97 00 | Variable Frequency Drives |
| 41 01 00 | Operation and Maintenance of Material Processing and Handling Equipment |
| 41 05 00 | Common Work Results for Material Processing and Handling Equipment |
| 41 06 00 | Schedules for Material Processing and Handling Equipment |
| 41 08 00 | Commissioning of Material Processing and Handling Equipment |
| 41 10 00 | Bulk Material Processing Equipment |
| 41 11 00 | Bulk Material Sizing Equipment |
| 41 12 00 | Bulk Material Conveying Equipment |
| 41 13 00 | Bulk Material Feeders |
| 41 14 00 | Batching Equipment |
| 41 20 00 | Piece Material Handling Equipment |
| 41 21 00 | Conveyors |
| 41 22 00 | Cranes and Hoists |
| 41 23 00 | Lifting Devices |
| 41 24 00 | Specialty Material Handling Equipment |
| 41 30 00 | Manufacturing Equipment |
| 41 31 00 | Manufacturing Lines and Equipment |
| 41 32 00 | Forming Equipment |
| 41 33 00 | Machining Equipment |
| 41 34 00 | Finishing Equipment |
| 41 35 00 | Dies and Molds |
| 41 36 00 | Assembly and Testing Equipment |
| 41 40 00 | Container Processing and Packaging |
| 41 41 00 | Container Filling and Sealing |
| 41 42 00 | Container Packing Equipment |
| 41 43 00 | Shipping Packaging |
| 41 50 00 | Material Storage |
| 41 51 00 | Automatic Material Storage |
| 41 52 00 | Bulk Material Storage |
| 41 53 00 | Storage Equipment and Systems |
| 41 60 00 | Mobile Plant Equipment |
| 41 61 00 | Mobile Earth Moving Equipment |
| 41 62 00 | Trucks |
| 41 63 00 | General Vehicles |
| 41 64 00 | Rail Vehicles |
| 41 65 00 | Mobile Support Equipment |
| 41 66 00 | Miscellaneous Mobile Equipment |
| 41 67 00 | Plant Maintenance Equipment |
| 42 01 00 | Operation and Maintenance of Process Heating, Cooling, and Drying Equipment |
| 42 05 00 | Common Work Results for Process Heating, Cooling, and Drying Equipment |
| 42 06 00 | Schedules for Process Heating, Cooling, and Drying Equipment |
| 42 08 00 | Commissioning of Process Heating, Cooling, and Drying Equipment |
| 42 10 00 | Process Heating Equipment |
| 42 11 00 | Process Boilers |
| 42 12 00 | Process Heaters |
| 42 13 00 | Industrial Heat Exchangers and Recuperators |
| 42 14 00 | Industrial Furnaces |
| 42 15 00 | Industrial Ovens |
| 42 20 00 | Process Cooling Equipment |
| 42 21 00 | Process Cooling Towers |
| 42 22 00 | Process Chillers and Coolers |
| 42 23 00 | Process Condensers and Evaporators |
| 42 30 00 | Process Drying Equipment |
| 42 31 00 | Gas Dryers and Dehumidifiers |
| 42 32 00 | Material Dryers |
| 43 01 00 | Operation and Maintenance of Process Gas and Liquid Handling, Purification, and Storage Equipment |
| 43 05 00 | Common Work Results for Process Gas and Liquid Handling Purification and Storage Equipment |
| 43 06 00 | Schedules for Process Gas and Liquid Handling, Purification, and Storage Equipment |
| 43 08 00 | Commissioning of Process Gas and Liquid Handling, Purification, and Storage Equipment |
| 43 10 00 | Gas Handling Equipment |
| 43 11 00 | Gas Fans, Blowers, Pumps and Boosters |
| 43 12 00 | Gas Compressors |
| 43 13 00 | Gas Process Equipment |
| 43 15 00 | Process Air and Gas Filters |
| 43 20 00 | Liquid Handling Equipment |
| 43 22 00 | Liquid Process Equipment |
| 43 23 00 | Dry Location Liquid Pumps |
| 43 24 00 | Suspended Liquid Pumps |
| 43 25 00 | Submersible/Immersible Liquid Pumps |
| 43 26 00 | Specialized Liquid Pumps |
| 43 27 00 | Process Liquid Filters |
| 43 30 00 | Gas and Liquid Purification Equipment |
| 43 31 00 | Gas and Liquid Purification Filtration Equipment |
| 43 32 00 | Gas and Liquid Purification Process Equipment |
| 43 40 00 | Gas and Liquid Storage |
| 43 41 00 | Non-pressurized Tanks and Vessels |
| 43 42 00 | Pressurized Tanks and Vessels |
| 44 01 00 | Operation and Maintenance of Pollution and Waste Control Equipment |
| 44 05 00 | Common Work Results for Pollution and Waste Control Equipment |
| 44 06 00 | Schedules for Pollution and Waste Control Equipment |
| 44 08 00 | Commissioning of Pollution and Waste Control Equipment |
| 44 10 00 | Air Pollution Control |
| 44 11 00 | Particulate Control Equipment |
| 44 13 00 | Gaseous Air Pollution Control Equipment |
| 44 20 00 | Noise Pollution Control |
| 44 21 00 | Noise Pollution Control Equipment |
| 44 30 00 | Odor Control |
| 44 31 00 | Odor Treatment Equipment |
| 44 32 00 | Odor Dispersing and Masking/Counteracting Equipment |
| 44 40 00 | Water Pollution Control Equipment |
| 44 41 00 | Water Pollution Containment and Cleanup Equipment |
| 44 50 00 | Solid Waste Control and Reuse |
| 44 51 00 | Solid Waste Collection, Transfer, and Hauling Equipment |
| 44 53 00 | Solid Waste Processing Equipment |
| 44 55 00 | Composting Equipment |
| 44 60 00 | Waste Thermal Processing Equipment |
| 44 61 00 | Waste-to-Energy Plants |
| 44 62 00 | Fluidized Bed Combustion Equipment |
| 44 63 00 | Rotary Kiln Incinerators |
| 44 64 00 | Gasification Equipment |
| 44 65 00 | Pyrolysis Equipment |
| 44 66 00 | Hazardous Waste and Medical Waste Incinerators |
| 44 67 00 | Heat Recovery Equipment for Waste Thermal Processing |
| 44 68 00 | Synthesis Gas Cleanup and Handling Equipment |
| 45 05 00 | Common Work Results for Industry-Specific Manufacturing Equipment |
| 45 08 00 | Commissioning of Industry-Specific Manufacturing Equipment |
| 45 11 00 | Oil and Gas Extraction Equipment |
| 45 12 00 | User-Defined Oil and Gas Extraction Equipment |
| 45 13 00 | Mining Machinery and Equipment |
| 45 14 00 | User-Defined Mining Machinery and Equipment |
| 45 15 00 | Food Manufacturing Equipment |
| 45 16 00 | User-Defined Food Manufacturing Equipment |
| 45 17 00 | Beverage and Tobacco Manufacturing Equipment |
| 45 18 00 | User-Defined Beverage and Tobacco Manufacturing Equipment |
| 45 19 00 | Textiles and Apparel Manufacturing Equipment |
| 45 20 00 | User-Defined Textiles and Apparel Manufacturing Equipment |
| 45 21 00 | Leather and Allied Product Manufacturing Equipment |
| 45 22 00 | User-Defined Leather and Allied Product Manufacturing Equipment |
| 45 23 00 | Wood Product Manufacturing Equipment |
| 45 24 00 | User-Defined Wood Product Manufacturing Equipment |
| 45 25 00 | Paper Manufacturing Equipment |
| 45 26 00 | User-Defined Paper Manufacturing Equipment |
| 45 27 00 | Printing and Related Manufacturing Equipment |
| 45 28 00 | User-Defined Printing and Related Manufacturing Equipment |
| 45 29 00 | Petroleum and Coal Products Manufacturing Equipment |
| 45 30 00 | User-Defined Petroleum and Coal Products Manufacturing Equipment |
| 45 31 00 | Chemical Manufacturing Equipment |
| 45 32 00 | User-Defined Chemical Manufacturing Equipment |
| 45 33 00 | Plastics and Rubber Manufacturing Equipment |
| 45 34 00 | User-Defined Plastics and Rubber Manufacturing Equipment |
| 45 35 00 | Nonmetallic Mineral Product Manufacturing Equipment |
| 45 36 00 | User-Defined Nonmetallic Mineral Product Manufacturing Equipment |
| 45 37 00 | Primary Metal Manufacturing Equipment |
| 45 38 00 | User-Defined Primary Metal Manufacturing Equipment |
| 45 39 00 | Fabricated Metal Product Manufacturing Equipment |
| 45 40 00 | User-Defined Fabricated Metal Product Manufacturing Equipment |
| 45 41 00 | Machinery Manufacturing Equipment |
| 45 42 00 | User-Defined Machinery Manufacturing Equipment |
| 45 43 00 | Computer and Electronic Product Manufacturing Equipment |
| 45 44 00 | User-Defined Computer and Electronic Product Manufacturing Equipment |
| 45 45 00 | Electrical Equipment, Appliance, and Component Manufacturing Equipment |
| 45 46 00 | User-Defined Electrical Equipment, Appliance, and Component Manufacturing Equipment |
| 45 47 00 | Transportation Manufacturing Equipment |
| 45 48 00 | User-Defined Transportation Manufacturing Equipment |
| 45 49 00 | Furniture and Related Product Manufacturing Equipment |
| 45 50 00 | User-Defined Furniture and Related Product Manufacturing Equipment |
| 45 52 00 | User-Defined Other Manufacturing Equipment |
| 45 53 00 | Biopharmaceutical Manufacturing Equipment |
| 46 01 00 | Operation and Maintenance of Water and Wastewater Equipment |
| 46 05 00 | Common Work Results for Water and Wastewater Equipment |
| 46 06 00 | Schedules for Water and Wastewater Equipment |
| 46 07 00 | Packaged Water and Wastewater Treatment Equipment |
| 46 08 00 | Commissioning of Water and Wastewater Equipment |
| 46 20 00 | Water and Wastewater Preliminary Treatment Equipment |
| 46 21 00 | Screening Equipment |
| 46 23 00 | Grit Removal and Handling Equipment |
| 46 24 00 | Grinding and Shredding Equipment |
| 46 25 00 | Oil and Grease Separation and Removal Equipment |
| 46 30 00 | Water and Wastewater Chemical Feed Equipment |
| 46 31 00 | Gas Chemical Feed Equipment |
| 46 33 00 | Liquid Chemical Feed Equipment |
| 46 36 00 | Dry Chemical Feed Equipment |
| 46 40 00 | Water and Wastewater Clarification and Mixing Equipment |
| 46 41 00 | Mixing Equipment |
| 46 43 00 | Clarifier Equipment |
| 46 46 00 | Sediment Removal Equipment |
| 46 50 00 | Water and Wastewater Secondary Treatment Equipment |
| 46 51 00 | Air and Gas Diffusion Equipment |
| 46 53 00 | Biological Treatment Systems |
| 46 60 00 | Water and Wastewater Advanced Treatment Equipment |
| 46 61 00 | Filtration Equipment |
| 46 63 00 | Demineralization Equipment |
| 46 66 00 | Ultraviolet Equipment |
| 46 70 00 | Water and Wastewater Residuals Handling and Treatment |
| 46 71 00 | Residuals Thickening Equipment |
| 46 73 00 | Residuals Stabilization |
| 46 76 00 | Residuals Dewatering Equipment |
| 46 78 00 | Thermal Treatment of Residuals |
| 48 01 00 | Operation and Maintenance for Electrical Power Generation |
| 48 05 00 | Common Work Results for Electrical Power Generation |
| 48 06 00 | Schedules for Electrical Power Generation |
| 48 08 00 | Commissioning of Electrical Power Generation |
| 48 09 00 | Instrumentation and Control for Electrical Power Generation |
| 48 10 00 | Electrical Power Generation Equipment |
| 48 11 00 | Fossil Fuel Plant Electrical Power Generation Equipment |
| 48 12 00 | Nuclear Fuel Plant Electrical Power Generation Equipment |
| 48 13 00 | Hydroelectric Plant Electrical Power Generation Equipment |
| 48 14 00 | Solar Energy Electrical Power Generation Equipment |
| 48 15 00 | Wind Energy Electrical Power Generation Equipment |
| 48 16 00 | Geothermal Energy Electrical Power Generation Equipment |
| 48 17 00 | Electrochemical Energy Electrical Power Generation Equipment |
| 48 18 00 | Fuel Cell Electrical Power Generation Equipment |
| 48 19 00 | Electrical Power Control Equipment |
| 48 30 00 | Combined Heat and Power Generation |
| 48 31 00 | Combustion Combined Heat and Power Generation |
| 48 70 00 | Electrical Power Generation Testing |
| 48 71 00 | Electrical Power Generation Test Equipment |
