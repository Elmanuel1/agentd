---
name: masterformat-building
description: "MasterFormat Level 2 codes for vendor groups. Covers building construction trades (divisions 02-14) and facility services (divisions 21-28): concrete, masonry, metals, wood/plastics, thermal/moisture, openings, finishes, specialties, equipment, furnishings, special construction, conveying, fire suppression, plumbing, HVAC, integrated automation, electrical, communications, and electronic safety/security."
---

# MasterFormat Level 2 Reference — Building Construction & Facility Services

Covers building construction trades (divisions 02-14) and facility services (divisions 21-28): concrete, masonry, metals, wood/plastics, thermal/moisture, openings, finishes, specialties, equipment, furnishings, special construction, conveying, fire suppression, plumbing, HVAC, integrated automation, electrical, communications, and electronic safety/security.

## How to Use MasterFormat Skills

**Always attempt a MasterFormat code lookup for every vendor group**, regardless of document format.

1. **Choose the right skill** based on work scope:
   - Building or facility specification (divisions 02-28) -> this skill (**masterformat-building**)
   - Civil, infrastructure, or site work (divisions 31-35) -> **masterformat-civil**
   - Process or industrial equipment (divisions 40-48) -> **masterformat-process**
   - Mixed scope -> read whichever skill best fits the majority, or read multiple skills

2. **If the document IS CSI MasterFormat** (section numbers like `07 50 00`, `03 30 00`, or "Division XX" headings):
   - Use the document's section codes directly as `masterformat_code`
   - Validate each code exists in the skill table. If a code is not in the table, set `masterformat_code: null`

3. **If the document is NOT CSI MasterFormat** (NatSpec, NBS, work packages, proprietary scope breakdowns, trade lists, etc.):
   - Look up the scope in the MasterFormat skill table and find the closest matching code
   - Example: a "Roofing" scope package -> look up roofing in this skill -> assign `"07 50 00"` (or the best-matching code)
   - If truly no code in the table matches the vendor group's scope, set `masterformat_code: null`

4. **If uncertain** about the best match -> set `masterformat_code: null`. Never force a match.

## Reference Table

Use this table to assign `masterformat_code` to each vendor group.
Pick the code whose title best matches the scope of work.
If no code fits, set `masterformat_code: null`.

| Code     | Title                                        |
|----------|----------------------------------------------|
| 02 01 00 | Maintenance of Existing Conditions |
| 02 03 00 | Conservation Treatment for Existing Period Conditions |
| 02 05 00 | Common Work Results for Existing Conditions |
| 02 06 00 | Schedules for Existing Conditions |
| 02 08 00 | Commissioning of Existing Conditions |
| 02 20 00 | Assessment |
| 02 21 00 | Surveys |
| 02 22 00 | Existing Conditions Assessment |
| 02 24 00 | Environmental Assessment |
| 02 25 00 | Existing Material Assessment |
| 02 26 00 | Hazardous Material Assessment |
| 02 30 00 | Subsurface Investigation |
| 02 31 00 | Geophysical Investigations |
| 02 32 00 | Geotechnical Investigations |
| 02 40 00 | Demolition and Structure Moving |
| 02 41 00 | Demolition |
| 02 42 00 | Removal and Salvage of Construction Materials |
| 02 43 00 | Structure Moving |
| 02 50 00 | Site Remediation |
| 02 51 00 | Physical Decontamination |
| 02 52 00 | Chemical Decontamination |
| 02 53 00 | Thermal Decontamination |
| 02 54 00 | Biological Decontamination |
| 02 55 00 | Remediation Soil Stabilization |
| 02 56 00 | Site Containment |
| 02 57 00 | Sinkhole Remediation |
| 02 58 00 | Snow Control |
| 02 60 00 | Contaminated Site Material Removal |
| 02 61 00 | Removal and Disposal of Contaminated Soils |
| 02 62 00 | Hazardous Waste Recovery Processes |
| 02 65 00 | Underground Storage Tank Removal |
| 02 70 00 | Water Remediation |
| 02 71 00 | Groundwater Treatment |
| 02 72 00 | Water Decontamination |
| 02 80 00 | Facility Remediation |
| 02 81 00 | Transportation and Disposal of Hazardous Materials |
| 02 82 00 | Asbestos Remediation |
| 02 83 00 | Lead Remediation |
| 02 84 00 | Polychlorinate Biphenyl Remediation |
| 02 86 00 | Hazardous Waste Drum Handling |
| 02 87 00 | Biohazard Remediation |
| 03 01 00 | Maintenance of Concrete |
| 03 03 00 | Conservation Treatment for Period Concrete |
| 03 05 00 | Common Work Results for Concrete |
| 03 06 00 | Schedules for Concrete |
| 03 31 00 | Structural Concrete |
| 03 33 00 | Architectural Concrete |
| 03 34 00 | Low Density Concrete |
| 03 35 00 | Concrete Finishing |
| 03 37 00 | Specialty Placed Concrete |
| 03 38 00 | Post-Tensioned Concrete |
| 03 39 00 | Concrete Curing |
| 03 40 00 | Precast Concrete |
| 03 41 00 | Precast Structural Concrete |
| 03 45 00 | Precast Architectural Concrete |
| 03 47 00 | Site-Cast Concrete |
| 03 48 00 | Precast Concrete Specialties |
| 03 49 00 | Glass-Fiber-Reinforced Concrete |
| 03 50 00 | Cast Decks and Underlayment |
| 03 51 00 | Cast Roof Decks |
| 03 52 00 | Lightweight Concrete Roof Insulation |
| 03 53 00 | Concrete Topping |
| 03 54 00 | Cast Underlayment |
| 03 60 00 | Grouting |
| 03 61 00 | Cementitious Grouting |
| 03 62 00 | Non-Shrink Grouting |
| 03 63 00 | Epoxy Grouting |
| 03 64 00 | Injection Grouting |
| 03 70 00 | Mass Concrete |
| 03 71 00 | Mass Concrete for Raft Foundations |
| 03 72 00 | Mass Concrete for Dams |
| 03 80 00 | Concrete Cutting and Boring |
| 03 81 00 | Concrete Cutting |
| 03 82 00 | Concrete Boring |
| 04 01 00 | Maintenance of Masonry |
| 04 03 00 | Conservation Treatment for Period Masonry |
| 04 05 00 | Common Work Results for Masonry |
| 04 06 00 | Schedules for Masonry |
| 04 08 00 | Commissioning of Masonry |
| 04 20 00 | Unit Masonry |
| 04 21 00 | Clay Unit Masonry |
| 04 22 00 | Concrete Unit Masonry |
| 04 23 00 | Glass Unit Masonry |
| 04 24 00 | Adobe Unit Masonry |
| 04 25 00 | Unit Masonry Panels |
| 04 26 00 | Single-Wythe Unit Masonry |
| 04 27 00 | Multiple-Wythe Unit Masonry |
| 04 28 00 | Concrete Form Masonry Units |
| 04 29 00 | Engineered Unit Masonry |
| 04 40 00 | Stone Assemblies |
| 04 41 00 | Dry-Placed Stone |
| 04 42 00 | Exterior Stone Cladding |
| 04 43 00 | Stone Masonry |
| 04 50 00 | Refractory Masonry |
| 04 51 00 | Flue Liner Masonry |
| 04 52 00 | Combustion Chamber Masonry |
| 04 53 00 | Castable Refractory Masonry |
| 04 54 00 | Refractory Brick Masonry |
| 04 57 00 | Masonry Fireplaces |
| 04 60 00 | Corrosion-Resistant Masonry |
| 04 61 00 | Chemical-Resistant Brick Masonry |
| 04 62 00 | Vitrified Clay Liner Plate |
| 04 70 00 | Manufactured Masonry |
| 04 71 00 | Manufactured Brick Masonry |
| 04 72 00 | Cast Stone Masonry |
| 04 73 00 | Manufactured Stone Masonry |
| 05 01 00 | Maintenance of Metals |
| 05 03 00 | Conservation Treatment for Period Metals |
| 05 05 00 | Common Work Results for Metals |
| 05 06 00 | Schedules for Metals |
| 05 08 00 | Commissioning of Metals |
| 05 10 00 | Structural Metal Framing |
| 05 12 00 | Structural Steel Framing |
| 05 13 00 | Structural Stainless-Steel Framing |
| 05 14 00 | Structural Aluminum Framing |
| 05 15 00 | Wire Rope Assemblies |
| 05 16 00 | Structural Cabling |
| 05 17 00 | Structural Rod Assemblies |
| 05 19 00 | Tension Rod and Cable Truss Assemblies |
| 05 20 00 | Metal Joists |
| 05 21 00 | Steel Joist Framing |
| 05 25 00 | Aluminum Joist Framing |
| 05 30 00 | Metal Decking |
| 05 31 00 | Steel Decking |
| 05 33 00 | Aluminum Decking |
| 05 34 00 | Acoustical Metal Decking |
| 05 35 00 | Raceway Decking Assemblies |
| 05 36 00 | Composite Metal Decking |
| 05 40 00 | Cold-Formed Metal Framing |
| 05 41 00 | Structural Metal Stud Framing |
| 05 42 00 | Cold-Formed Metal Joist Framing |
| 05 43 00 | Slotted Channel Framing |
| 05 44 00 | Cold-Formed Metal Trusses |
| 05 45 00 | Metal Support Assemblies |
| 05 50 00 | Metal Fabrications |
| 05 51 00 | Metal Stairs |
| 05 52 00 | Metal Railings |
| 05 53 00 | Metal Gratings |
| 05 54 00 | Metal Floor Plates |
| 05 55 00 | Metal Stair Treads and Nosings |
| 05 56 00 | Metal Castings |
| 05 58 00 | Formed Metal Fabrications |
| 05 59 00 | Metal Specialties |
| 05 70 00 | Decorative Metal |
| 05 71 00 | Decorative Metal Stairs |
| 05 73 00 | Decorative Metal Railings |
| 05 74 00 | Decorative Metal Castings |
| 05 75 00 | Decorative Formed Metal |
| 05 76 00 | Decorative Forged Metal |
| 05 77 00 | Decorative Extruded Metal |
| 06 01 00 | Maintenance of Wood, Plastics, and Composites |
| 06 03 00 | Conservation Treatment for Period Wood |
| 06 05 00 | Common Work Results for Wood, Plastics, and Composites |
| 06 06 00 | Schedules for Wood, Plastics, and Composites |
| 06 08 00 | Commissioning of Wood, Plastics, and Composites |
| 06 10 00 | Rough Carpentry |
| 06 11 00 | Wood Framing |
| 06 12 00 | Structural Panels |
| 06 13 00 | Heavy Timber Construction |
| 06 14 00 | Treated Wood Foundations |
| 06 15 00 | Wood Decking |
| 06 16 00 | Sheathing |
| 06 17 00 | Shop-Fabricated Structural Wood |
| 06 18 00 | Glued-Laminated Construction |
| 06 20 00 | Finish Carpentry |
| 06 22 00 | Millwork |
| 06 25 00 | Prefinished Paneling |
| 06 26 00 | Board Paneling |
| 06 40 00 | Architectural Woodwork |
| 06 41 00 | Architectural Wood Casework |
| 06 42 00 | Wood Paneling |
| 06 43 00 | Wood Stairs and Railings |
| 06 44 00 | Ornamental Woodwork |
| 06 46 00 | Wood Trim |
| 06 48 00 | Wood Frames |
| 06 49 00 | Wood Screens and Shutters |
| 06 50 00 | Structural Plastics |
| 06 51 00 | Structural Plastic Shapes and Plates |
| 06 52 00 | Plastic Structural Assemblies |
| 06 53 00 | Plastic Decking |
| 06 60 00 | Plastic Fabrications |
| 06 61 00 | Cast Polymer Fabrications |
| 06 63 00 | Plastic Railings |
| 06 64 00 | Plastic Paneling |
| 06 65 00 | Plastic Trim |
| 06 66 00 | Custom Ornamental Simulated Woodwork |
| 06 70 00 | Structural Composites |
| 06 71 00 | Structural Composite Shapes and Plates |
| 06 72 00 | Composite Structural Assemblies |
| 06 73 00 | Composite Decking |
| 06 74 00 | Composite Gratings |
| 06 80 00 | Composite Fabrications |
| 06 81 00 | Composite Railings |
| 06 82 00 | Composite Trim |
| 06 83 00 | Composite Paneling |
| 07 01 00 | Operation and Maintenance of Thermal and Moisture Protection |
| 07 03 00 | Conservation Treatment for Period Roofing |
| 07 05 00 | Common Work Results for Thermal and Moisture Protection |
| 07 06 00 | Schedules for Thermal and Moisture Protection |
| 07 08 00 | Commissioning of Thermal and Moisture Protection |
| 07 10 00 | Dampproofing and Waterproofing |
| 07 11 00 | Dampproofing |
| 07 12 00 | Built-Up Bituminous Waterproofing |
| 07 13 00 | Sheet Waterproofing |
| 07 14 00 | Fluid-Applied Waterproofing |
| 07 15 00 | Sheet Metal Waterproofing |
| 07 16 00 | Cementitious and Reactive Waterproofing |
| 07 17 00 | Bentonite Waterproofing |
| 07 18 00 | Traffic Coatings |
| 07 19 00 | Water Repellents |
| 07 20 00 | Thermal Protection |
| 07 21 00 | Thermal Insulation |
| 07 22 00 | Roof and Deck Insulation |
| 07 24 00 | Exterior Insulation and Finish Systems |
| 07 25 00 | Weather Barriers |
| 07 26 00 | Vapor Retarders |
| 07 27 00 | Air Barriers |
| 07 30 00 | Steep Slope Roofing |
| 07 31 00 | Shingles and Shakes |
| 07 32 00 | Roof Tiles |
| 07 33 00 | Natural Roof Coverings |
| 07 40 00 | Roofing and Siding Panels |
| 07 41 00 | Roof Panels |
| 07 42 00 | Wall Panels |
| 07 44 00 | Faced Panels |
| 07 46 00 | Siding |
| 07 50 00 | Membrane Roofing |
| 07 51 00 | Built-Up Bituminous Roofing |
| 07 52 00 | Modified Bituminous Membrane Roofing |
| 07 53 00 | Elastomeric Membrane Roofing |
| 07 54 00 | Thermoplastic Membrane Roofing |
| 07 55 00 | Protected Membrane Roofing |
| 07 56 00 | Fluid-Applied Roofing |
| 07 57 00 | Coated Foamed Roofing |
| 07 58 00 | Roll Roofing |
| 07 60 00 | Flashing and Sheet Metal |
| 07 61 00 | Sheet Metal Roofing |
| 07 62 00 | Sheet Metal Flashing and Trim |
| 07 63 00 | Sheet Metal Roofing Specialties |
| 07 64 00 | Sheet Metal Wall Cladding |
| 07 65 00 | Flexible Flashing |
| 07 70 00 | Roof and Wall Specialties and Accessories |
| 07 71 00 | Roof Specialties |
| 07 72 00 | Roof Accessories |
| 07 76 00 | Roof Pavers |
| 07 77 00 | Wall Specialties |
| 07 80 00 | Fire and Smoke Protection |
| 07 81 00 | Applied Fireproofing |
| 07 82 00 | Board Fireproofing |
| 07 84 00 | Firestopping |
| 07 86 00 | Smoke Seals |
| 07 87 00 | Smoke Containment Barriers |
| 07 90 00 | Joint Protection |
| 07 91 00 | Preformed Joint Seals |
| 07 92 00 | Joint Sealants |
| 07 95 00 | Expansion Control |
| 08 01 00 | Operation and Maintenance of Openings |
| 08 03 00 | Conservation Treatment for Period Openings |
| 08 05 00 | Common Work Results for Openings |
| 08 06 00 | Schedules for Openings |
| 08 08 00 | Commissioning of Openings |
| 08 10 00 | Doors and Frames |
| 08 11 00 | Metal Doors and Frames |
| 08 12 00 | Metal Frames |
| 08 13 00 | Metal Doors |
| 08 14 00 | Wood Doors |
| 08 15 00 | Plastic Doors |
| 08 16 00 | Composite Doors |
| 08 17 00 | Integrated Door Opening Assemblies |
| 08 30 00 | Specialty Doors and Frames |
| 08 31 00 | Access Doors and Panels |
| 08 32 00 | Sliding Glass Doors |
| 08 33 00 | Coiling Doors and Grilles |
| 08 34 00 | Special Function Doors |
| 08 35 00 | Folding Doors and Grilles |
| 08 36 00 | Panel Doors |
| 08 38 00 | Traffic Doors |
| 08 39 00 | Pressure-Resistant Doors |
| 08 40 00 | Entrances, Storefronts, and Curtain Walls |
| 08 41 00 | Entrances and Storefronts |
| 08 42 00 | Entrances |
| 08 43 00 | Storefronts |
| 08 44 00 | Curtain Wall and Glazed Assemblies |
| 08 45 00 | Translucent Wall and Roof Assemblies |
| 08 46 00 | Window Wall Assemblies |
| 08 50 00 | Windows |
| 08 51 00 | Metal Windows |
| 08 52 00 | Wood Windows |
| 08 53 00 | Plastic Windows |
| 08 54 00 | Composite Windows |
| 08 55 00 | Pressure-Resistant Windows |
| 08 56 00 | Special Function Windows |
| 08 60 00 | Roof Windows and Skylights |
| 08 61 00 | Roof Windows |
| 08 62 00 | Unit Skylights |
| 08 63 00 | Metal-Framed Skylights |
| 08 64 00 | Plastic-Framed Skylights |
| 08 65 00 | Glazed Canopies |
| 08 67 00 | Skylight Protection and Screens |
| 08 70 00 | Hardware |
| 08 71 00 | Door Hardware |
| 08 75 00 | Window Hardware |
| 08 78 00 | Special Function Hardware |
| 08 79 00 | Hardware Accessories |
| 08 80 00 | Glazing |
| 08 81 00 | Glass Glazing |
| 08 83 00 | Mirrors |
| 08 84 00 | Plastic Glazing |
| 08 85 00 | Glazing Accessories |
| 08 87 00 | Glazing Surface Films |
| 08 88 00 | Special Function Glazing |
| 08 90 00 | Louvers and Vents |
| 08 91 00 | Louvers |
| 08 92 00 | Louvered Equipment Enclosures |
| 08 95 00 | Vents |
| 09 01 00 | Maintenance of Finishes |
| 09 03 00 | Conservation Treatment of Period Finishes |
| 09 05 00 | Common Work Results for Finishes |
| 09 06 00 | Schedules for Finishes |
| 09 08 00 | Commissioning of Finishes |
| 09 20 00 | Plaster and Gypsum Board |
| 09 21 00 | Plaster and Gypsum Board Assemblies |
| 09 22 00 | Supports for Plaster and Gypsum Board |
| 09 23 00 | Gypsum Plastering |
| 09 24 00 | Cement Plastering |
| 09 25 00 | Other Plastering |
| 09 26 00 | Veneer Plastering |
| 09 27 00 | Plaster Fabrications |
| 09 28 00 | Backing Boards and Underlayments |
| 09 29 00 | Gypsum Board |
| 09 30 00 | Tiling |
| 09 31 00 | Thin-Set Tiling |
| 09 32 00 | Mortar-Bed Tiling |
| 09 33 00 | Conductive Tiling |
| 09 34 00 | Waterproofing-Membrane Tiling |
| 09 35 00 | Chemical-Resistant Tiling |
| 09 50 00 | Ceilings |
| 09 51 00 | Acoustical Ceilings |
| 09 53 00 | Acoustical Ceiling Suspension Assemblies |
| 09 54 00 | Specialty Ceilings |
| 09 56 00 | Textured Ceilings |
| 09 57 00 | Special Function Ceilings |
| 09 58 00 | Integrated Ceiling Assemblies |
| 09 60 00 | Flooring |
| 09 61 00 | Flooring Treatment |
| 09 62 00 | Specialty Flooring |
| 09 63 00 | Masonry Flooring |
| 09 64 00 | Wood Flooring |
| 09 65 00 | Resilient Flooring |
| 09 66 00 | Terrazzo Flooring |
| 09 67 00 | Fluid-Applied Flooring |
| 09 68 00 | Carpeting |
| 09 69 00 | Access Flooring |
| 09 70 00 | Wall Finishes |
| 09 72 00 | Wall Coverings |
| 09 73 00 | Wall Carpeting |
| 09 74 00 | Flexible Wood Sheets |
| 09 75 00 | Stone Facing |
| 09 76 00 | Plastic Blocks |
| 09 77 00 | Special Wall Surfacing |
| 09 78 00 | Interior Wall Paneling |
| 09 80 00 | Acoustic Treatment |
| 09 81 00 | Acoustic Insulation |
| 09 83 00 | Acoustic Finishes |
| 09 84 00 | Acoustic Room Components |
| 09 90 00 | Painting and Coating |
| 09 91 00 | Painting |
| 09 93 00 | Staining and Transparent Finishing |
| 09 94 00 | Decorative Finishing |
| 09 96 00 | High-Performance Coatings |
| 09 97 00 | Special Coatings |
| 10 01 00 | Operation and Maintenance of Specialties |
| 10 05 00 | Common Work Results for Specialties |
| 10 06 00 | Schedules for Specialties |
| 10 08 00 | Commissioning of Specialties |
| 10 10 00 | Information Specialties |
| 10 11 00 | Visual Display Units |
| 10 12 00 | Display Cases |
| 10 13 00 | Directories |
| 10 14 00 | Signage |
| 10 17 00 | Telephone Specialties |
| 10 18 00 | Informational Kiosks |
| 10 20 00 | Interior Specialties |
| 10 21 00 | Compartments and Cubicles |
| 10 22 00 | Partitions |
| 10 25 00 | Service Walls |
| 10 26 00 | Wall and Door Protection |
| 10 28 00 | Toilet, Bath, and Laundry Accessories |
| 10 30 00 | Fireplaces and Stoves |
| 10 31 00 | Manufactured Fireplaces |
| 10 32 00 | Fireplace Specialties |
| 10 35 00 | Stoves |
| 10 40 00 | Safety Specialties |
| 10 41 00 | Emergency Access and Information Cabinets |
| 10 43 00 | Emergency Aid Specialties |
| 10 44 00 | Fire Protection Specialties |
| 10 45 00 | Photoluminescent Exit Specialties |
| 10 50 00 | Storage Specialties |
| 10 51 00 | Lockers |
| 10 55 00 | Postal Specialties |
| 10 56 00 | Storage Assemblies |
| 10 57 00 | Wardrobe and Closet Specialties |
| 10 70 00 | Exterior Specialties |
| 10 71 00 | Exterior Protection |
| 10 73 00 | Protective Covers |
| 10 74 00 | Manufactured Exterior Specialties |
| 10 75 00 | Flagpoles |
| 10 80 00 | Other Specialties |
| 10 81 00 | Pest Control Devices |
| 10 82 00 | Grilles and Screens |
| 10 83 00 | Flags and Banners |
| 10 84 00 | Gas Lighting |
| 10 86 00 | Security Mirrors and Domes |
| 10 88 00 | Scales |
| 11 01 00 | Operation and Maintenance of Equipment |
| 11 05 00 | Common Work Results for Equipment |
| 11 06 00 | Schedules for Equipment |
| 11 08 00 | Commissioning of Equipment |
| 11 10 00 | Vehicle and Pedestrian Equipment |
| 11 11 00 | Vehicle Service Equipment |
| 11 12 00 | Parking Control Equipment |
| 11 13 00 | Loading Dock Equipment |
| 11 14 00 | Pedestrian Control Equipment |
| 11 15 00 | Security Control Equipment |
| 11 20 00 | Commercial Equipment |
| 11 21 00 | Retail and Service Equipment |
| 11 22 00 | Banking Equipment |
| 11 25 00 | Hospitality Equipment |
| 11 28 00 | Office Equipment |
| 11 29 00 | Postal, Packaging, and Shipping Equipment |
| 11 30 00 | Residential Equipment |
| 11 32 00 | Unit Kitchens |
| 11 40 00 | Foodservice Equipment |
| 11 41 00 | Foodservice Storage Equipment |
| 11 42 00 | Food Preparation Equipment |
| 11 43 00 | Food Delivery Carts and Conveyors |
| 11 44 00 | Food Cooking Equipment |
| 11 46 00 | Food Dispensing Equipment |
| 11 48 00 | Foodservice Cleaning and Disposal Equipment |
| 11 50 00 | Educational and Scientific Equipment |
| 11 51 00 | Library Equipment |
| 11 52 00 | Audio-Visual Equipment |
| 11 53 00 | Laboratory Equipment |
| 11 55 00 | Planetarium Equipment |
| 11 56 00 | Observatory Equipment |
| 11 57 00 | Vocational Shop Equipment |
| 11 59 00 | Exhibit Equipment |
| 11 60 00 | Entertainment and Recreation Equipment |
| 11 61 00 | Broadcast, Theater, and Stage Equipment |
| 11 62 00 | Musical Equipment |
| 11 66 00 | Athletic Equipment |
| 11 67 00 | Recreational Equipment |
| 11 68 00 | Play Field Equipment and Structures |
| 11 70 00 | Healthcare Equipment |
| 11 71 00 | Medical Sterilizing Equipment |
| 11 72 00 | Examination and Treatment Equipment |
| 11 73 00 | Patient Care Equipment |
| 11 74 00 | Dental Equipment |
| 11 75 00 | Optical Equipment |
| 11 76 00 | Operating Room Equipment |
| 11 77 00 | Radiology Equipment |
| 11 78 00 | Mortuary Equipment |
| 11 79 00 | Therapy Equipment |
| 11 80 00 | Facility Maintenance and Operation Equipment |
| 11 81 00 | Facility Maintenance Equipment |
| 11 82 00 | Facility Solid Waste Handling Equipment |
| 11 90 00 | Other Equipment |
| 11 91 00 | Religious Equipment |
| 11 92 00 | Agricultural Equipment |
| 11 93 00 | Horticultural Equipment |
| 11 94 00 | Veterinary Equipment |
| 11 95 00 | Arts and Crafts Equipment |
| 11 97 00 | Security Equipment |
| 11 98 00 | Detention Equipment |
| 12 01 00 | Operation and Maintenance of Furnishings |
| 12 05 00 | Common Work Results for Furnishings |
| 12 06 00 | Schedules for Furnishings |
| 12 08 00 | Commissioning of Furnishings |
| 12 10 00 | Art |
| 12 11 00 | Murals |
| 12 12 00 | Wall Decorations |
| 12 14 00 | Sculptures |
| 12 17 00 | Art Glass |
| 12 19 00 | Religious Art |
| 12 20 00 | Window Treatments |
| 12 21 00 | Window Blinds |
| 12 22 00 | Curtains and Drapes |
| 12 23 00 | Interior Shutters |
| 12 24 00 | Window Shades |
| 12 25 00 | Window Treatment Operating Hardware |
| 12 26 00 | Interior Daylighting Devices |
| 12 30 00 | Casework |
| 12 31 00 | Manufactured Metal Casework |
| 12 32 00 | Manufactured Wood Casework |
| 12 34 00 | Manufactured Plastic Casework |
| 12 35 00 | Specialty Casework |
| 12 36 00 | Countertops |
| 12 40 00 | Furnishings and Accessories |
| 12 41 00 | Office Accessories |
| 12 42 00 | Table Accessories |
| 12 43 00 | Portable Lamps |
| 12 44 00 | Bath Furnishings |
| 12 45 00 | Bedroom Furnishings |
| 12 46 00 | Furnishing Accessories |
| 12 48 00 | Rugs and Mats |
| 12 50 00 | Furniture |
| 12 51 00 | Office Furniture |
| 12 52 00 | Seating |
| 12 53 00 | Retail Furniture |
| 12 54 00 | Hospitality Furniture |
| 12 55 00 | Detention Furniture |
| 12 56 00 | Institutional Furniture |
| 12 57 00 | Industrial Furniture |
| 12 58 00 | Residential Furniture |
| 12 59 00 | Systems Furniture |
| 12 60 00 | Multiple Seating |
| 12 61 00 | Fixed Audience Seating |
| 12 62 00 | Portable Audience Seating |
| 12 63 00 | Stadium and Arena Seating |
| 12 64 00 | Booths and Tables |
| 12 65 00 | Multiple-Use Fixed Seating |
| 12 66 00 | Telescoping Stands |
| 12 67 00 | Pews and Benches |
| 12 68 00 | Seat and Table Assemblies |
| 12 90 00 | Other Furnishings |
| 12 92 00 | Interior Planters and Artificial Plants |
| 12 93 00 | Interior Public Space Furnishings |
| 13 01 00 | Operation and Maintenance of Special Construction |
| 13 05 00 | Common Work Results for Special Construction |
| 13 06 00 | Schedules for Special Construction |
| 13 08 00 | Commissioning of Special Construction |
| 13 10 00 | Special Facility Components |
| 13 11 00 | Swimming Pools |
| 13 12 00 | Fountains |
| 13 13 00 | Aquariums |
| 13 14 00 | Amusement Park Structures and Equipment |
| 13 15 00 | Specialty Element Construction |
| 13 17 00 | Tubs and Pools |
| 13 18 00 | Ice Rinks |
| 13 19 00 | Kennels and Animal Shelters |
| 13 20 00 | Special Purpose Rooms |
| 13 21 00 | Controlled Environment Rooms |
| 13 22 00 | Office Shelters and Booths |
| 13 23 00 | Planetariums |
| 13 24 00 | Special Activity Rooms |
| 13 26 00 | Fabricated Rooms |
| 13 27 00 | Vaults |
| 13 28 00 | Athletic and Recreational Special Construction |
| 13 30 00 | Special Structures |
| 13 31 00 | Fabric Structures |
| 13 32 00 | Space Frames |
| 13 33 00 | Geodesic Structures |
| 13 34 00 | Fabricated Engineered Structures |
| 13 35 00 | Rammed Earth Construction |
| 13 36 00 | Towers |
| 13 40 00 | Integrated Construction |
| 13 42 00 | Building Modules |
| 13 44 00 | Modular Mezzanines |
| 13 47 00 | Facility Protection |
| 13 48 00 | Sound, Vibration, and Seismic Control |
| 13 49 00 | Radiation Protection |
| 13 50 00 | Special Instrumentation |
| 13 51 00 | Stress Instrumentation |
| 13 52 00 | Seismic Instrumentation |
| 13 53 00 | Meteorological Instrumentation |
| 14 01 00 | Operation and Maintenance of Conveying Equipment |
| 14 05 00 | Common Work Results for Conveying Equipment |
| 14 06 00 | Schedules for Conveying Equipment |
| 14 08 00 | Commissioning of Conveying Equipment |
| 14 10 00 | Dumbwaiters |
| 14 11 00 | Manual Dumbwaiters |
| 14 12 00 | Electric Dumbwaiters |
| 14 14 00 | Hydraulic Dumbwaiters |
| 14 20 00 | Elevators |
| 14 21 00 | Electric Traction Elevators |
| 14 24 00 | Hydraulic Elevators |
| 14 26 00 | Limited-Use/Limited-Application Elevators |
| 14 27 00 | Custom Elevator Cabs and Doors |
| 14 28 00 | Elevator Equipment and Controls |
| 14 30 00 | Escalators and Moving Walks |
| 14 31 00 | Escalators |
| 14 32 00 | Moving Walks |
| 14 33 00 | Moving Ramps |
| 14 40 00 | Lifts |
| 14 41 00 | People Lifts |
| 14 42 00 | Wheelchair Lifts |
| 14 43 00 | Platform Lifts |
| 14 44 00 | Sidewalk Lifts |
| 14 45 00 | Vehicle Lifts |
| 14 46 00 | Material Lifts |
| 14 70 00 | Turntables |
| 14 71 00 | Industrial Turntables |
| 14 72 00 | Hospitality Turntables |
| 14 73 00 | Exhibit Turntables |
| 14 74 00 | Entertainment Turntables |
| 14 80 00 | Scaffolding |
| 14 81 00 | Suspended Scaffolding |
| 14 82 00 | Rope Climbers |
| 14 83 00 | Elevating Platforms |
| 14 84 00 | Powered Scaffolding |
| 14 90 00 | Other Conveying Equipment |
| 14 91 00 | Facility Chutes |
| 14 92 00 | Pneumatic Tube Systems |
| 14 93 00 | Slide Pole Systems |
| 21 01 00 | Operation and Maintenance of Fire Suppression |
| 21 05 00 | Common Work Results for Fire Suppression |
| 21 06 00 | Schedules for Fire Suppression |
| 21 07 00 | Fire Suppression Systems Insulation |
| 21 08 00 | Commissioning of Fire Suppression |
| 21 09 00 | Instrumentation and Control for Fire-Suppression Systems |
| 21 10 00 | Water-Based Fire-Suppression Systems |
| 21 11 00 | Facility Fire-Suppression Water-Service Piping |
| 21 12 00 | Fire-Suppression Standpipes |
| 21 13 00 | Fire-Suppression Sprinkler Systems |
| 21 16 00 | Fire-Suppression Pressure Maintenance Pumps |
| 21 20 00 | Fire-Extinguishing Systems |
| 21 21 00 | Carbon-Dioxide Fire-Extinguishing Systems |
| 21 22 00 | Clean-Agent Fire-Extinguishing Systems |
| 21 23 00 | Wet-Chemical Fire-Extinguishing Systems |
| 21 24 00 | Dry-Chemical Fire-Extinguishing Systems |
| 21 30 00 | Fire Pumps |
| 21 31 00 | Centrifugal Fire Pumps |
| 21 32 00 | Vertical-Turbine Fire Pumps |
| 21 33 00 | Positive-Displacement Fire Pumps |
| 21 34 00 | Fire Pump Accessories |
| 21 40 00 | Fire-Suppression Water Storage |
| 21 41 00 | Storage Tanks for Fire-Suppression Water |
| 22 01 00 | Operation and Maintenance of Plumbing |
| 22 05 00 | Common Work Results for Plumbing |
| 22 06 00 | Schedules for Plumbing |
| 22 07 00 | Plumbing Insulation |
| 22 08 00 | Commissioning of Plumbing |
| 22 09 00 | Instrumentation and Control for Plumbing |
| 22 10 00 | Plumbing Piping |
| 22 11 00 | Facility Water Distribution |
| 22 12 00 | Facility Potable-Water Storage Tanks |
| 22 13 00 | Facility Sanitary Sewerage |
| 22 14 00 | Facility Storm Drainage |
| 22 15 00 | General Service Compressed-Air Systems |
| 22 30 00 | Plumbing Equipment |
| 22 31 00 | Domestic Water Softeners |
| 22 32 00 | Domestic Water Filtration Equipment |
| 22 33 00 | Electric Domestic Water Heaters |
| 22 34 00 | Fuel-Fired Domestic Water Heaters |
| 22 35 00 | Domestic Water Heat Exchangers |
| 22 36 00 | Domestic Water Preheaters |
| 22 40 00 | Plumbing Fixtures |
| 22 41 00 | Residential Plumbing Fixtures |
| 22 42 00 | Commercial Plumbing Fixtures |
| 22 43 00 | Healthcare Plumbing Fixtures |
| 22 45 00 | Emergency Plumbing Fixtures |
| 22 46 00 | Security Plumbing Fixtures |
| 22 47 00 | Drinking Fountains and Water Coolers |
| 22 50 00 | Pool and Fountain Plumbing Systems |
| 22 51 00 | Swimming Pool Plumbing Systems |
| 22 52 00 | Fountain Plumbing Systems |
| 22 60 00 | Gas and Vacuum Systems for Laboratory and Healthcare Facilities |
| 22 61 00 | Compressed-Air Systems for Laboratory and Healthcare Facilities |
| 22 62 00 | Vacuum Systems for Laboratory and Healthcare Facilities |
| 22 63 00 | Gas Systems for Laboratory and Healthcare Facilities |
| 22 66 00 | Chemical-Waste Systems for Laboratory and Healthcare Facilities |
| 22 67 00 | Processed Water Systems for Laboratory and Healthcare Facilities |
| 23 01 00 | Operation and Maintenance of HVAC Systems |
| 23 05 00 | Common Work Results for HVAC |
| 23 06 00 | Schedules for HVAC |
| 23 07 00 | HVAC Insulation |
| 23 08 00 | Commissioning of HVAC |
| 23 09 00 | Instrumentation and Control for HVAC |
| 23 10 00 | Facility Fuel Systems |
| 23 11 00 | Facility Fuel Piping |
| 23 12 00 | Facility Fuel Pumps |
| 23 13 00 | Facility Fuel-Storage Tanks |
| 23 20 00 | HVAC Piping and Pumps |
| 23 21 00 | Hydronic Piping and Pumps |
| 23 22 00 | Steam and Condensate Piping and Pumps |
| 23 23 00 | Refrigerant Piping |
| 23 24 00 | Internal-Combustion Engine Piping |
| 23 25 00 | HVAC Water Treatment |
| 23 30 00 | HVAC Air Distribution |
| 23 31 00 | HVAC Ducts and Casings |
| 23 32 00 | Air Plenums and Chases |
| 23 33 00 | Air Duct Accessories |
| 23 34 00 | HVAC Fans |
| 23 35 00 | Special Exhaust Systems |
| 23 36 00 | Air Terminal Units |
| 23 37 00 | Air Outlets and Inlets |
| 23 38 00 | Ventilation Hoods |
| 23 40 00 | HVAC Air Cleaning Devices |
| 23 41 00 | Particulate Air Filtration |
| 23 42 00 | Gas-Phase Air Filtration |
| 23 43 00 | Electronic Air Cleaners |
| 23 50 00 | Central Heating Equipment |
| 23 51 00 | Breechings, Chimneys, and Stacks |
| 23 52 00 | Heating Boilers |
| 23 53 00 | Heating Boiler Feedwater Equipment |
| 23 54 00 | Furnaces |
| 23 55 00 | Fuel-Fired Heaters |
| 23 56 00 | Solar Energy Heating Equipment |
| 23 57 00 | Heat Exchangers for HVAC |
| 23 60 00 | Central Cooling Equipment |
| 23 61 00 | Refrigerant Compressors |
| 23 62 00 | Packaged Compressor and Condenser Units |
| 23 63 00 | Refrigerant Condensers |
| 23 64 00 | Packaged Water Chillers |
| 23 65 00 | Cooling Towers |
| 23 70 00 | Central HVAC Equipment |
| 23 71 00 | Thermal Storage |
| 23 72 00 | Air-to-Air Energy Recovery Equipment |
| 23 73 00 | Indoor Central-Station Air-Handling Units |
| 23 74 00 | Packaged Outdoor HVAC Equipment |
| 23 75 00 | Custom-Packaged Outdoor HVAC Equipment |
| 23 76 00 | Evaporative Air-Cooling Equipment |
| 23 80 00 | Decentralized HVAC Equipment |
| 23 81 00 | Decentralized Unitary HVAC Equipment |
| 23 82 00 | Convection Heating and Cooling Units |
| 23 83 00 | Radiant Heating Units |
| 23 84 00 | Humidity Control Equipment |
| 25 01 00 | Operation and Maintenance of Integrated Automation |
| 25 05 00 | Common Work Results for Integrated Automation |
| 25 06 00 | Schedules for Integrated Automation |
| 25 08 00 | Commissioning of Integrated Automation |
| 25 10 00 | Integrated Automation Network Equipment |
| 25 11 00 | Integrated Automation Network Devices |
| 25 12 00 | Integrated Automation Network Gateways |
| 25 13 00 | Integrated Automation Control and Monitoring Network |
| 25 14 00 | Integrated Automation Local Control Units |
| 25 15 00 | Integrated Automation Software |
| 25 30 00 | Integrated Automation Instrumentation and Terminal Devices |
| 25 31 00 | Integrated Automation Instrumentation and Terminal Devices for Facility Equipment |
| 25 32 00 | Integrated Automation Instrumentation and Terminal Devices for Conveying Equipment |
| 25 33 00 | Integrated Automation Instrumentation and Terminal Devices for Fire-Suppression Systems |
| 25 34 00 | Integrated Automation Instrumentation and Terminal Devices for Plumbing |
| 25 35 00 | Integrated Automation Instrumentation and Terminal Devices for HVAC |
| 25 36 00 | Integrated Automation Instrumentation and Terminal Devices for Electrical Systems |
| 25 37 00 | Integrated Automation Instrumentation and Terminal Devices for Communications Systems |
| 25 38 00 | Integrated Automation Instrumentation and Terminal Devices for Electronic Safety and Security Systems |
| 25 50 00 | Integrated Automation Facility Controls |
| 25 51 00 | Integrated Automation Control of Facility Equipment |
| 25 52 00 | Integrated Automation Control of Conveying Equipment |
| 25 53 00 | Integrated Automation Control of Fire-Suppression Systems |
| 25 54 00 | Integrated Automation Control of Plumbing |
| 25 55 00 | Integrated Automation Control of HVAC |
| 25 56 00 | Integrated Automation Control of Electrical Systems |
| 25 57 00 | Integrated Automation Control of Communications Systems |
| 25 58 00 | Integrated Automation Control of Electronic Safety and Security Systems |
| 25 90 00 | Integrated Automation Control Sequences |
| 25 91 00 | Integrated Automation Control Sequences for Facility Equipment |
| 25 92 00 | Integrated Automation Control Sequences for Conveying Equipment |
| 25 93 00 | Integrated Automation Control Sequences for Fire-Suppression Systems |
| 25 94 00 | Integrated Automation Control Sequences for Plumbing |
| 25 95 00 | Integrated Automation Control Sequences for HVAC |
| 25 96 00 | Integrated Automation Control Sequences for Electrical Systems |
| 25 97 00 | Integrated Automation Control Sequences for Communications Systems |
| 25 98 00 | Integrated Automation Control Sequences for Electronic Safety and Security Systems |
| 26 01 00 | Operation and Maintenance of Electrical Systems |
| 26 05 00 | Common Work Results for Electrical |
| 26 06 00 | Schedules for Electrical |
| 26 08 00 | Commissioning of Electrical Systems |
| 26 09 00 | Instrumentation and Control for Electrical Systems |
| 26 10 00 | Medium-Voltage Electrical Distribution |
| 26 11 00 | Substations |
| 26 12 00 | Medium-Voltage Transformers |
| 26 13 00 | Medium-Voltage Switchgear |
| 26 15 00 | Medium-Voltage Enclosed Bus Assemblies |
| 26 16 00 | Medium-Voltage Metering |
| 26 18 00 | Medium-Voltage Circuit Protection Devices |
| 26 20 00 | Low-Voltage Electrical Distribution |
| 26 21 00 | Low-Voltage Electrical Service Entrance |
| 26 22 00 | Low-Voltage Transformers |
| 26 23 00 | Low-Voltage Switchgear |
| 26 24 00 | Switchboards and Panelboards |
| 26 25 00 | Low-Voltage Enclosed Bus Assemblies |
| 26 27 00 | Low-Voltage Distribution Equipment |
| 26 28 00 | Low-Voltage Circuit Protective Devices |
| 26 29 00 | Low-Voltage Controllers |
| 26 30 00 | Facility Electrical Power Generating and Storing Equipment |
| 26 31 00 | Photovoltaic Collectors |
| 26 32 00 | Packaged Generator Assemblies |
| 26 33 00 | Battery Equipment |
| 26 35 00 | Power Filters and Conditioners |
| 26 36 00 | Transfer Switches |
| 26 40 00 | Electrical Protection |
| 26 41 00 | Facility Lightning Protection |
| 26 43 00 | Surge Protective Devices |
| 26 50 00 | Lighting |
| 26 51 00 | Interior Lighting |
| 26 52 00 | Safety Lighting |
| 26 54 00 | Classified Location Lighting |
| 26 55 00 | Special Purpose Lighting |
| 26 56 00 | Exterior Lighting |
| 27 01 00 | Operation and Maintenance of Communications Systems |
| 27 05 00 | Common Work Results for Communications |
| 27 06 00 | Schedules for Communications |
| 27 08 00 | Commissioning of Communications |
| 27 10 00 | Structured Cabling |
| 27 11 00 | Communications Equipment Room Fittings |
| 27 13 00 | Communications Backbone Cabling |
| 27 15 00 | Communications Horizontal Cabling |
| 27 16 00 | Communications Connecting Cords, Devices, and Adapters |
| 27 20 00 | Data Communications |
| 27 21 00 | Data Communications Network Equipment |
| 27 22 00 | Data Communications Hardware |
| 27 24 00 | Data Communications Peripheral Data Equipment |
| 27 25 00 | Data Communications Software |
| 27 26 00 | Data Communications Programming and Integration Services |
| 27 30 00 | Voice Communications |
| 27 31 00 | Voice Communications Switching and Routing Equipment |
| 27 32 00 | Voice Communications Terminal Equipment |
| 27 33 00 | Voice Communications Messaging |
| 27 34 00 | Call Accounting |
| 27 35 00 | Call Management |
| 27 40 00 | Audio-Video Communications |
| 27 41 00 | Audio-Video Systems |
| 27 42 00 | Electronic Digital Systems |
| 27 50 00 | Distributed Communications and Monitoring Systems |
| 27 51 00 | Distributed Audio-Video Communications Systems |
| 27 52 00 | Healthcare Communications and Monitoring Systems |
| 27 53 00 | Distributed Systems |
| 28 01 00 | Operation and Maintenance of Electronic Safety and Security |
| 28 05 00 | Common Work Results for Electronic Safety and Security |
| 28 06 00 | Schedules for Electronic Safety and Security |
| 28 08 00 | Commissioning of Electronic Safety and Security |
| 28 10 00 | Access Control |
| 28 11 00 | Access Control Global Applications |
| 28 13 00 | Access Control Software and Database Management |
| 28 14 00 | Access Control System Hardware |
| 28 15 00 | Access Control Hardware Devices |
| 28 16 00 | Access Control Interfaces |
| 28 17 00 | Access Control Identification Management Systems |
| 28 18 00 | Security Access Detection Equipment |
| 28 19 00 | Access Control Vehicle Identification Systems |
| 28 20 00 | Video Surveillance |
| 28 21 00 | Surveillance Cameras |
| 28 23 00 | Video Management System |
| 28 25 00 | Video Surveillance Positioning Equipment |
| 28 27 00 | Video Surveillance Sensors |
| 28 30 00 | Security Detection, Alarm, and Monitoring |
| 28 31 00 | Intrusion Detection |
| 28 33 00 | Security Monitoring and Control |
| 28 35 00 | Tracking Systems |
| 28 37 00 | Audio Monitoring |
| 28 40 00 | Life Safety |
| 28 41 00 | Radiation Detection and Alarm |
| 28 42 00 | Gas Detection and Alarm |
| 28 43 00 | Fuel Oil Detection and Alarm |
| 28 44 00 | Refrigerant Detection and Alarm |
| 28 45 00 | Water Detection and Alarm |
| 28 46 00 | Fire Detection and Alarm |
| 28 47 00 | Mass Notification |
| 28 48 00 | Emergency Response Systems |
| 28 49 00 | Electronic Personal Protection Systems |
| 28 50 00 | Specialized Systems |
| 28 51 00 | Information Management & Presentation |
| 28 52 00 | Detention Security Systems |
