---
name: masterformat-civil
description: "MasterFormat Level 2 codes for vendor groups. Covers site and infrastructure work (divisions 31-35): earthwork, exterior improvements, utilities, transportation, and waterway/marine construction."
---

# MasterFormat Level 2 Reference — Civil & Infrastructure

Covers site and infrastructure work (divisions 31-35): earthwork, exterior improvements, utilities, transportation, and waterway/marine construction.

## How to Use MasterFormat Skills

**Always attempt a MasterFormat code lookup for every vendor group**, regardless of document format.

1. **Choose the right skill** based on work scope:
   - Building or facility specification (divisions 02-28) -> **masterformat-building**
   - Civil, infrastructure, or site work (divisions 31-35) -> this skill (**masterformat-civil**)
   - Process or industrial equipment (divisions 40-48) -> **masterformat-process**
   - Mixed scope -> read whichever skill best fits the majority, or read multiple skills

2. **If the document IS CSI MasterFormat** (section numbers like `31 23 00`, `33 10 00`, or "Division XX" headings):
   - Use the document's section codes directly as `masterformat_code`
   - Validate each code exists in the skill table. If a code is not in the table, set `masterformat_code: null`

3. **If the document is NOT CSI MasterFormat** (NatSpec, NBS, work packages, proprietary scope breakdowns, trade lists, etc.):
   - Look up the scope in the MasterFormat skill table and find the closest matching code
   - Example: an "Earthwork" scope package -> look up earthwork in this skill -> assign `"31 20 00"` (or the best-matching code)
   - If truly no code in the table matches the vendor group's scope, set `masterformat_code: null`

4. **If uncertain** about the best match -> set `masterformat_code: null`. Never force a match.

## Reference Table

Use this table to assign `masterformat_code` to each vendor group.
Pick the code whose title best matches the scope of work.
If no code fits, set `masterformat_code: null`.

| Code     | Title                                        |
|----------|----------------------------------------------|
| 31 01 00 | Maintenance of Earthwork |
| 31 05 00 | Common Work Results for Earthwork |
| 31 06 00 | Schedules for Earthwork |
| 31 08 00 | Commissioning of Earthwork |
| 31 09 00 | Geotechnical Instrumentation and Monitoring of Earthwork |
| 31 10 00 | Site Clearing |
| 31 11 00 | Clearing and Grubbing |
| 31 12 00 | Selective Clearing |
| 31 13 00 | Selective Tree and Shrub Removal and Trimming |
| 31 14 00 | Earth Stripping and Stockpiling |
| 31 20 00 | Earth Moving |
| 31 21 00 | Off-Gassing Mitigation |
| 31 22 00 | Grading |
| 31 23 00 | Excavation and Fill |
| 31 24 00 | Embankments |
| 31 25 00 | Erosion and Sedimentation Controls |
| 31 30 00 | Earthwork Methods |
| 31 31 00 | Soil Treatment |
| 31 32 00 | Soil Stabilization |
| 31 33 00 | Rock Stabilization |
| 31 34 00 | Soil Reinforcement |
| 31 35 00 | Slope Protection |
| 31 36 00 | Gabions |
| 31 37 00 | Riprap |
| 31 40 00 | Shoring and Underpinning |
| 31 41 00 | Shoring |
| 31 43 00 | Concrete Raising |
| 31 45 00 | Vibroflotation and Densification |
| 31 46 00 | Needle Beams |
| 31 48 00 | Underpinning |
| 31 50 00 | Excavation Support and Protection |
| 31 51 00 | Anchor Tiebacks |
| 31 52 00 | Cofferdams |
| 31 53 00 | Cribbing and Walers |
| 31 54 00 | Ground Freezing |
| 31 56 00 | Slurry Walls |
| 31 60 00 | Special Foundations and Load-Bearing Elements |
| 31 62 00 | Driven Piles |
| 31 63 00 | Bored Piles |
| 31 64 00 | Caissons |
| 31 66 00 | Special Foundations |
| 31 68 00 | Foundation Anchors |
| 31 70 00 | Tunneling and Mining |
| 31 71 00 | Tunnel Excavation |
| 31 72 00 | Tunnel Support Systems |
| 31 73 00 | Tunnel Grouting |
| 31 74 00 | Tunnel Construction |
| 31 75 00 | Shaft Construction |
| 31 77 00 | Submersible Tube Tunnels |
| 32 01 00 | Operation and Maintenance of Exterior Improvements |
| 32 05 00 | Common Work Results for Exterior Improvements |
| 32 06 00 | Schedules for Exterior Improvements |
| 32 08 00 | Commissioning of Exterior Improvements |
| 32 10 00 | Bases, Ballasts, and Paving |
| 32 11 00 | Base Courses |
| 32 12 00 | Flexible Paving |
| 32 13 00 | Rigid Paving |
| 32 14 00 | Unit Paving |
| 32 15 00 | Aggregate Surfacing |
| 32 16 00 | Curbs, Gutters, Sidewalks, and Driveways |
| 32 17 00 | Paving Specialties |
| 32 18 00 | Athletic and Recreational Surfacing |
| 32 30 00 | Site Improvements |
| 32 31 00 | Fences and Gates |
| 32 32 00 | Retaining Walls |
| 32 33 00 | Site Furnishings |
| 32 34 00 | Fabricated Bridges |
| 32 35 00 | Screening Devices |
| 32 39 00 | Manufactured Site Specialties |
| 32 70 00 | Wetlands |
| 32 71 00 | Constructed Wetlands |
| 32 72 00 | Wetlands Restoration |
| 32 80 00 | Irrigation |
| 32 81 00 | Irrigation Components |
| 32 82 00 | Irrigation Pumps |
| 32 84 00 | Planting Irrigation |
| 32 86 00 | Agricultural Irrigation |
| 32 90 00 | Planting |
| 32 91 00 | Planting Preparation |
| 32 92 00 | Turf and Grasses |
| 32 93 00 | Plants |
| 32 94 00 | Planting Accessories |
| 32 95 00 | Exterior Planting Support Structures |
| 32 96 00 | Transplanting |
| 33 01 00 | Operation and Maintenance of Utilities |
| 33 05 00 | Common Work Results for Utilities |
| 33 06 00 | Schedules for Utilities |
| 33 08 00 | Commissioning of Utilities |
| 33 09 00 | Instrumentation and Control for Utilities |
| 33 10 00 | Water Utilities |
| 33 11 00 | Groundwater Sources |
| 33 12 00 | Surface Water Sources |
| 33 14 00 | Water Utility Transmission and Distribution |
| 33 16 00 | Water Utility Storage Tanks |
| 33 19 00 | Water Utility Metering Equipment |
| 33 30 00 | Sanitary Sewerage |
| 33 31 00 | Sanitary Sewerage Piping |
| 33 32 00 | Sanitary Sewerage Equipment |
| 33 34 00 | Onsite Wastewater Disposal |
| 33 36 00 | Wastewater Utility Storage Tanks |
| 33 37 00 | Overflow Control |
| 33 40 00 | Stormwater Utilities |
| 33 41 00 | Subdrainage |
| 33 42 00 | Stormwater Conveyance |
| 33 44 00 | Stormwater Utility Equipment |
| 33 46 00 | Stormwater Management |
| 33 50 00 | Hydrocarbon Utilities |
| 33 51 00 | Hydrocarbon Sources |
| 33 52 00 | Hydrocarbon Transmission and Distribution |
| 33 53 00 | Hydrocarbon Utility Pressure Regulation |
| 33 54 00 | Hydrocarbon Utility Safety Equipment |
| 33 56 00 | Hydrocarbon Storage |
| 33 57 00 | Bulk Hydrocarbons Receiving/Dispensing Equipment |
| 33 59 00 | Hydrocarbon Utility Metering |
| 33 60 00 | Hydronic and Steam Energy Utilities |
| 33 61 00 | Hydronic Energy Distribution |
| 33 63 00 | Steam Energy Distribution |
| 33 70 00 | Electrical Utilities |
| 33 71 00 | Electrical Utility Transmission and Distribution |
| 33 72 00 | Utility Substations |
| 33 73 00 | Utility Transformers |
| 33 74 00 | Extra High Voltage (EHV) Switchgear and protection devices |
| 33 75 00 | High-Voltage Switchgear and Protection Devices |
| 33 77 00 | Medium-Voltage Utility Switchgear and Protection Devices |
| 33 78 00 | Substation Converter Stations |
| 33 79 00 | Site Grounding |
| 33 80 00 | Communications Utilities |
| 33 81 00 | Communications Structures |
| 33 82 00 | Communications Transmission and Distribution |
| 33 83 00 | Wireless Communications Transmission and Distribution |
| 34 01 00 | Operation and Maintenance of Transportation |
| 34 05 00 | Common Work Results for Transportation |
| 34 06 00 | Schedules for Transportation |
| 34 08 00 | Commissioning of Transportation |
| 34 10 00 | Guideways/Railways |
| 34 11 00 | Rail Tracks |
| 34 12 00 | Monorails |
| 34 13 00 | Funiculars |
| 34 14 00 | Cable Transportation |
| 34 20 00 | Traction Power |
| 34 21 00 | Traction Power Distribution |
| 34 23 00 | Overhead Traction Power |
| 34 24 00 | Third Rail Traction Power |
| 34 40 00 | Transportation Signaling and Control Equipment |
| 34 41 00 | Roadway Signaling and Control Equipment |
| 34 42 00 | Railway Signaling and Control Equipment |
| 34 43 00 | Airfield Signaling and Control Equipment |
| 34 48 00 | Bridge Signaling and Control Equipment |
| 34 50 00 | Transportation Fare Collection Equipment |
| 34 52 00 | Vehicle Fare Collection |
| 34 54 00 | Passenger Fare Collection |
| 34 70 00 | Transportation Construction and Equipment |
| 34 71 00 | Roadway Construction |
| 34 72 00 | Railway Construction |
| 34 73 00 | Airfield Construction |
| 34 75 00 | Roadway Equipment |
| 34 76 00 | Railway Equipment |
| 34 77 00 | Transportation Equipment |
| 34 78 00 | Weighing Equipment |
| 34 80 00 | Bridges |
| 34 81 00 | Bridge Machinery |
| 34 82 00 | Bridge Specialties |
| 35 01 00 | Operation and Maintenance of Waterway and Marine Construction |
| 35 05 00 | Common Work Results for Waterway and Marine Construction |
| 35 06 00 | Schedules for Waterway and Marine Construction |
| 35 08 00 | Commissioning of Waterway and Marine Construction |
| 35 10 00 | Waterway and Marine Signaling and Control Equipment |
| 35 11 00 | Signaling and Control Equipment for Waterways |
| 35 12 00 | Marine Signaling and Control Equipment |
| 35 13 00 | Signaling and Control Equipment for Dams |
| 35 20 00 | Waterway and Marine Construction and Equipment |
| 35 21 00 | General Fabrications for Waterways |
| 35 22 00 | Hydraulic Gates |
| 35 24 00 | Dredging |
| 35 30 00 | Coastal Construction |
| 35 31 00 | Shoreline Protection |
| 35 32 00 | Artificial Reefs |
| 35 40 00 | Waterway Construction and Equipment |
| 35 41 00 | Levees |
| 35 42 00 | Waterway Bank Protection |
| 35 43 00 | Waterway Scour Protection |
| 35 49 00 | Waterway Structures |
| 35 50 00 | Marine Construction and Equipment |
| 35 51 00 | Floating Construction |
| 35 52 00 | Offshore Platform Construction |
| 35 53 00 | Underwater Construction |
| 35 59 00 | Marine Specialties |
| 35 70 00 | Dam Construction and Equipment |
| 35 71 00 | Gravity Dams |
| 35 72 00 | Arch Dams |
| 35 73 00 | Embankment Dams |
| 35 74 00 | Buttress Dams |
| 35 79 00 | Auxiliary Dam Structures |
