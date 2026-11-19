#![allow(clippy::unwrap_used)]

use super::*;

/// A STEP file around `data` with `unit` as the length unit (instance #900).
fn step(unit: &str, data: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('test'),'2;1');\nFILE_NAME('part.stp','2026-11-19',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{unit}\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
    )
}

const MM: &str = "#900=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));";

/// The eight corners of a box from (x0, y0, z0) to (x1, y1, z1) as vertex points.
fn box_vertices(a: [f64; 3], b: [f64; 3]) -> String {
    let mut out = String::new();
    let mut id = 1;
    for x in [a[0], b[0]] {
        for y in [a[1], b[1]] {
            for z in [a[2], b[2]] {
                out.push_str(&format!(
                    "#{id}=CARTESIAN_POINT('',({x:?},{y:?},{z:?}));\n#{}=VERTEX_POINT('',#{id});\n",
                    id + 1
                ));
                id += 2;
            }
        }
    }
    out
}

fn size(text: &str) -> (ModelSize, Vec<String>) {
    match model_size(text) {
        Ok(found) => found,
        Err(err) => unreachable!("a usable model: {err}"),
    }
}

#[test]
fn a_box_in_millimetres_gives_its_width_height_and_depth() {
    let (s, warnings) = size(&step(MM, &box_vertices([-10.0, 5.0, 0.0], [12.5, 95.0, 70.0])));
    assert_eq!(
        s,
        ModelSize {
            width: 22.5,
            height: 90.0,
            depth: 70.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn metres_and_inches_are_turned_into_millimetres() {
    let metres = "#900=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT($,.METRE.));";
    let (s, _) = size(&step(metres, &box_vertices([0.0; 3], [0.0225, 0.09, 0.07])));
    assert!((s.width - 22.5).abs() < 1e-9 && (s.height - 90.0).abs() < 1e-9 && (s.depth - 70.0).abs() < 1e-9);

    // The millimetre has the lower id, but the geometry's context assigns the inch.
    let inches = "#900=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n\
                  #901=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#900);\n\
                  #902=DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\n\
                  #903=(CONVERSION_BASED_UNIT('INCH',#901)LENGTH_UNIT()NAMED_UNIT(#902));\n\
                  #904=(PLANE_ANGLE_UNIT()NAMED_UNIT(*)SI_UNIT($,.RADIAN.));\n\
                  #905=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#903,#904))REPRESENTATION_CONTEXT('',''));";
    let (s, _) = size(&step(inches, &box_vertices([0.0; 3], [1.0, 2.0, 3.0])));
    assert!((s.width - 25.4).abs() < 1e-9 && (s.height - 50.8).abs() < 1e-9 && (s.depth - 76.2).abs() < 1e-9);

    // A second, unrelated context in millimetres with a lower id, holding a representation of
    // its own: the inch context of the representation that holds the geometry still wins.
    let competing = "#880=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#881))REPRESENTATION_CONTEXT('',''));\n\
                     #881=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n\
                     #17=AXIS2_PLACEMENT_3D('',#1,$,$);\n\
                     #882=SHAPE_REPRESENTATION('other',(#17),#880);\n";
    let shape = "#990=ADVANCED_BREP_SHAPE_REPRESENTATION('part',(#2),#905);";
    let (s, _) = size(&step(
        &format!("{competing}{inches}\n{shape}"),
        &box_vertices([0.0; 3], [1.0, 2.0, 3.0]),
    ));
    assert!((s.width - 25.4).abs() < 1e-9 && (s.height - 50.8).abs() < 1e-9 && (s.depth - 76.2).abs() < 1e-9);
    // Loose points only: the representation that holds them picks the inch, not one that only
    // holds a placement.
    let loose = "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=CARTESIAN_POINT('',(1.,2.,3.));\n\
                 #30=CARTESIAN_POINT('',(0.,0.,0.));#31=AXIS2_PLACEMENT_3D('',#30,$,$);\n\
                 #880=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#881))REPRESENTATION_CONTEXT('',''));\n\
                 #881=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n\
                 #882=SHAPE_REPRESENTATION('other',(#31),#880);\n\
                 #990=SHAPE_REPRESENTATION('points',(#1,#2),#905);";
    let (s, _) = size(&step(inches, loose));
    assert!((s.width - 25.4).abs() < 1e-9 && (s.depth - 76.2).abs() < 1e-9, "{s:?}");
    // A 2D vertex in the millimetre context is not measured, so it does not pick the unit.
    let flat = loose
        .replace(
            "#882=SHAPE_REPRESENTATION('other',(#31),#880);",
            "#4=CARTESIAN_POINT('',(0.,0.));#5=VERTEX_POINT('',#4);\n#882=SHAPE_REPRESENTATION('other',(#5),#880);",
        )
        .replace(
            "GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#881))",
            "GEOMETRIC_REPRESENTATION_CONTEXT(2)GLOBAL_UNIT_ASSIGNED_CONTEXT((#881))",
        );
    assert!(flat.contains("#5=VERTEX_POINT") && flat.contains("CONTEXT(2)"));
    let (s, _) = size(&step(inches, &flat));
    assert!((s.width - 25.4).abs() < 1e-9 && (s.depth - 76.2).abs() < 1e-9, "{s:?}");
    // And the other way round: the shape in the millimetre context is read in millimetres.
    let shape_mm = "#990=ADVANCED_BREP_SHAPE_REPRESENTATION('part',(#2),#880);";
    let (s, _) = size(&step(
        &format!("{competing}{inches}\n{shape_mm}"),
        &box_vertices([0.0; 3], [1.0, 2.0, 3.0]),
    ));
    assert!((s.width - 1.0).abs() < 1e-9 && (s.depth - 3.0).abs() < 1e-9);

    // A unit only known by its measure.
    let custom = "#900=(CONVERSION_BASED_UNIT('thou',#901)LENGTH_UNIT()NAMED_UNIT(*));\n\
                  #901=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(0.0254),#903);\n\
                  #903=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));";
    let (s, _) = size(&step(custom, &box_vertices([0.0; 3], [1000.0, 1000.0, 1000.0])));
    assert!((s.width - 25.4).abs() < 1e-9);
}

#[test]
fn a_cylinder_counts_its_whole_round_side() {
    // A 10 mm round pin, 40 mm long along z: its only vertices are on the seam at x = 5.
    let data = "#1=CARTESIAN_POINT('',(5.,0.,0.));#2=VERTEX_POINT('',#1);\n\
                #3=CARTESIAN_POINT('',(5.,0.,40.));#4=VERTEX_POINT('',#3);\n\
                #5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(0.,0.,1.));#7=DIRECTION('',(1.,0.,0.));\n\
                #8=AXIS2_PLACEMENT_3D('',#5,#6,#7);#9=CIRCLE('',#8,5.);\n\
                #10=CARTESIAN_POINT('',(0.,0.,40.));#11=AXIS2_PLACEMENT_3D('',#10,#6,#7);#12=CIRCLE('',#11,5.);";
    let (s, _) = size(&step(MM, data));
    assert!((s.width - 10.0).abs() < 1e-9, "{s:?}");
    assert!((s.height - 10.0).abs() < 1e-9, "{s:?}");
    assert!((s.depth - 40.0).abs() < 1e-9, "{s:?}");

    // An ellipse in a tilted plane spans each axis by its two semi-axes as they lie there: the
    // first (4) along the reference direction, here x made square to the axis, the second (2)
    // along y.
    let tilted = "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=VERTEX_POINT('',#1);\n\
                  #5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(1.,0.,1.));\n\
                  #8=AXIS2_PLACEMENT_3D('',#5,#6,$);#9=ELLIPSE('',#8,4.,2.);";
    let (s, _) = size(&step(MM, tilted));
    let side = 2.0 * 4.0 * (0.5f64).sqrt();
    assert!(
        (s.width - side).abs() < 1e-9 && (s.height - 4.0).abs() < 1e-9 && (s.depth - side).abs() < 1e-9,
        "{s:?}"
    );

    // A flat ellipse with its long axis along y by its reference direction: 10 x 40 mm, not 40
    // x 40.
    let turned = "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=VERTEX_POINT('',#1);#3=VERTEX_POINT('',#20);\n\
                  #20=CARTESIAN_POINT('',(0.,0.,3.));\n\
                  #5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(0.,0.,1.));#7=DIRECTION('',(0.,1.,0.));\n\
                  #8=AXIS2_PLACEMENT_3D('',#5,#6,#7);#9=ELLIPSE('',#8,20.,5.);";
    let (s, _) = size(&step(MM, turned));
    assert!(
        (s.width - 10.0).abs() < 1e-9 && (s.height - 40.0).abs() < 1e-9 && (s.depth - 3.0).abs() < 1e-9,
        "{s:?}"
    );
}

#[test]
fn a_wireframe_circle_without_vertices_is_measured() {
    // A circle of radius 5 whose plane is square to (1, 1, 1): no vertex, and its centre is a
    // placement origin.
    let data = "#5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(1.,1.,1.));\n\
                #8=AXIS2_PLACEMENT_3D('',#5,#6,$);#9=CIRCLE('',#8,5.);";
    let (s, _) = size(&step(MM, data));
    let side = 2.0 * 5.0 * (2.0f64 / 3.0).sqrt();
    for v in [s.width, s.height, s.depth] {
        assert!((v - side).abs() < 1e-9, "{s:?}");
    }
}

#[test]
fn a_wireframe_measures_its_polylines_next_to_its_circles() {
    let data = "#5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(1.,1.,1.));\n\
                #8=AXIS2_PLACEMENT_3D('',#5,#6,$);#9=CIRCLE('',#8,5.);\n\
                #20=CARTESIAN_POINT('',(0.,0.,0.));#21=CARTESIAN_POINT('',(100.,20.,30.));\n\
                #22=POLYLINE('',(#20,#21));";
    let (s, _) = size(&step(MM, data));
    let r = 5.0 * (2.0f64 / 3.0).sqrt();
    assert!((s.width - (100.0 + r)).abs() < 1e-9, "{s:?}");
    assert!((s.height - (20.0 + r)).abs() < 1e-9, "{s:?}");
    assert!((s.depth - (30.0 + r)).abs() < 1e-9, "{s:?}");
}

#[test]
fn directions_in_tiny_ratios_keep_their_way() {
    let turned = |y: &str| {
        format!(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=VERTEX_POINT('',#1);#3=VERTEX_POINT('',#20);\n\
             #20=CARTESIAN_POINT('',(0.,0.,3.));\n\
             #5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(0.,0.,1E-20));#7=DIRECTION('',(0.,{y},0.));\n\
             #8=AXIS2_PLACEMENT_3D('',#5,#6,#7);#9=ELLIPSE('',#8,20.,5.);"
        )
    };
    for y in ["1.", "1E-13", "1E+200"] {
        let (s, _) = size(&step(MM, &turned(y)));
        assert!(
            (s.width - 10.0).abs() < 1e-9 && (s.height - 40.0).abs() < 1e-9 && (s.depth - 3.0).abs() < 1e-9,
            "{y}: {s:?}"
        );
    }
    // A reference direction all but along the axis still points the long side along y.
    let nearly = "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=VERTEX_POINT('',#1);#3=VERTEX_POINT('',#20);\n\
                  #20=CARTESIAN_POINT('',(0.,0.,3.));\n\
                  #5=CARTESIAN_POINT('',(0.,0.,0.));#6=DIRECTION('',(0.,0.,1.));#7=DIRECTION('',(0.,1E-10,1.));\n\
                  #8=AXIS2_PLACEMENT_3D('',#5,#6,#7);#9=ELLIPSE('',#8,20.,5.);";
    let (s, _) = size(&step(MM, nearly));
    assert!(
        (s.width - 10.0).abs() < 1e-9 && (s.height - 40.0).abs() < 1e-9 && (s.depth - 3.0).abs() < 1e-9,
        "{s:?}"
    );
    // Closer still, past where rounding could tell: still taken, and reported.
    let (s, warnings) = size(&step(MM, &nearly.replace("1E-10", "1E-15")));
    assert!(
        (s.width - 10.0).abs() < 1e-9 && (s.height - 40.0).abs() < 1e-9 && (s.depth - 3.0).abs() < 1e-9,
        "{s:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("all but along their axis")),
        "{warnings:?}"
    );
}

#[test]
fn a_file_cut_short_is_refused() {
    let whole = step(MM, &box_vertices([0.0; 3], [10.0, 20.0, 30.0]));
    assert!(model_size(&whole).is_ok());
    // Cut after enough vertices for a smaller box.
    let Some(cut) = whole.find("#9=") else {
        unreachable!("the box has a ninth instance")
    };
    let Err(err) = model_size(&whole[..cut]) else {
        unreachable!("a truncated exchange is refused")
    };
    assert!(err.to_string().contains("incomplete"), "{err}");
}

#[test]
fn a_b_spline_edge_counts_its_control_points() {
    let mut data = box_vertices([0.0; 3], [10.0, 10.0, 10.0]);
    data.push_str(
        "#100=CARTESIAN_POINT('',(0.,0.,0.));#101=CARTESIAN_POINT('',(5.,-4.,0.));#102=CARTESIAN_POINT('',(10.,0.,0.));\n\
         #103=B_SPLINE_CURVE_WITH_KNOTS('',2,(#100,#101,#102),.UNSPECIFIED.,.F.,.F.,(3,3),(0.,1.),.UNSPECIFIED.);\n\
         #104=CARTESIAN_POINT('',(0.,0.,12.));\n\
         #105=(BOUNDED_CURVE()B_SPLINE_CURVE(2,(#100,#104,#102),.UNSPECIFIED.,.F.,.F.)B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.)CURVE()GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE((1.,1.,1.))REPRESENTATION_ITEM(''));",
    );
    let (s, _) = size(&step(MM, &data));
    assert_eq!((s.width, s.height, s.depth), (10.0, 14.0, 12.0));
}

#[test]
fn placement_origins_and_far_construction_points_are_not_the_part() {
    // With vertices, a placement far away does not count.
    let mut data = box_vertices([0.0; 3], [10.0, 20.0, 30.0]);
    data.push_str("#100=CARTESIAN_POINT('',(500.,500.,500.));#101=AXIS2_PLACEMENT_3D('',#100,$,$);");
    assert_eq!(size(&step(MM, &data)).0.width, 10.0);

    // A tessellated model is measured by its coordinates.
    let tess = "#1=COORDINATES_LIST('',3,((0.,0.,0.),(6.,0.,0.),(0.,7.,8.)));\n\
                #2=CARTESIAN_POINT('',(900.,0.,0.));#3=AXIS2_PLACEMENT_3D('',#2,$,$);";
    let (s, _) = size(&step(MM, tess));
    assert_eq!((s.width, s.height, s.depth), (6.0, 7.0, 8.0));

    // Loose points without the placement origins.
    let loose = "#1=CARTESIAN_POINT('',(1.,2.,3.));#2=CARTESIAN_POINT('',(4.,6.,8.));\n\
                 #3=CARTESIAN_POINT('',(900.,0.,0.));#4=AXIS2_PLACEMENT_3D('',#3,$,$);";
    let (s, _) = size(&step(MM, loose));
    assert_eq!((s.width, s.height, s.depth), (3.0, 4.0, 5.0));
}

#[test]
fn strings_and_comments_do_not_split_entities() {
    let data = format!(
        "/* a comment; with (brackets) */\n#500=PRODUCT('it''s; a (part)','x;y','',(#501));\n{}",
        box_vertices([0.0; 3], [1.0, 2.0, 3.0])
    );
    let (s, warnings) = size(&step(MM, &data));
    assert_eq!((s.width, s.height, s.depth), (1.0, 2.0, 3.0));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn warnings_name_a_missing_unit_assemblies_and_unread_entities() {
    let mut data = box_vertices([0.0; 3], [1.0, 2.0, 3.0]);
    data.push_str("#200=ITEM_DEFINED_TRANSFORMATION('','',#1,#3);\n#201=CARTESIAN_POINT('',(1.,2.);\n");
    let (_, warnings) = size(&step("", &data));
    let all = warnings.join("\n");
    assert!(all.contains("millimetres are assumed"), "{all}");
    assert!(all.contains("assembly (1 placed parts)"), "{all}");
    assert!(all.contains("1 entities could not be read"), "{all}");
}

#[test]
fn files_without_a_usable_model_are_refused() {
    for text in [
        String::new(),
        "not a step file".to_owned(),
        step(MM, ""),
        // Flat: no depth.
        step(MM, &box_vertices([0.0; 3], [10.0, 10.0, 0.0])),
        // Far too big.
        step(MM, &box_vertices([0.0; 3], [10.0, 10.0, 1e6])),
        // Only 2D points.
        step(MM, "#1=CARTESIAN_POINT('',(1.,2.));#2=CARTESIAN_POINT('',(5.,9.));"),
        step(MM, "#1=CARTESIAN_POINT('',(1.,2.,NaN));"),
    ] {
        assert!(model_size(&text).is_err(), "{text}");
    }
}

#[test]
fn hostile_text_is_refused_without_a_panic() {
    let deep = format!(
        "#1=CARTESIAN_POINT('',{}1.{});",
        "(".repeat(100_000),
        ")".repeat(100_000)
    );
    for data in [
        deep,
        "#1=CARTESIAN_POINT('".to_owned(),
        "#1=CARTESIAN_POINT(\"abc".to_owned(),
        "#99999999999999999999999=CARTESIAN_POINT('',(1.,2.,3.));".to_owned(),
        "#1=(LENGTH_UNIT()CONVERSION_BASED_UNIT('x',#1));#2=CARTESIAN_POINT('',(1.,2.,3.));".to_owned(),
        "#1=VERTEX_POINT('',#1);#2=CIRCLE('',#2,5.);".to_owned(),
    ] {
        assert!(model_size(&step(MM, &data)).is_err(), "{}", &data[..data.len().min(80)]);
    }
}

/// One 10 x 20 x 5 mm box drawn as its own part (representation #300, origin #320), and an
/// assembly (#400) with `placements`, `uses` and further entities in `rest`.
fn assembly(uses: &[&str], placements: &str, rest: &str) -> String {
    let part = box_vertices([0.0; 3], [10.0, 20.0, 5.0]);
    let vertices: Vec<String> = (1..=8).map(|k| format!("#{}", 2 * k)).collect();
    step(
        MM,
        &format!(
            "{part}#321=CARTESIAN_POINT('',(0.,0.,0.));\n#320=AXIS2_PLACEMENT_3D('',#321,$,$);\n\
             #901=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#900)) REPRESENTATION_CONTEXT('',''));\n\
             #300=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#320,{}),#901);\n\
             #400=SHAPE_REPRESENTATION('',({}),#901);\n{placements}{rest}",
            vertices.join(","),
            uses.join(",")
        ),
    )
}

/// An axis placement `id` at `at`, with axis `z` and reference direction `x`.
fn axis(id: u32, at: [f64; 3], z: [f64; 3], x: [f64; 3]) -> String {
    format!(
        "#{id}=AXIS2_PLACEMENT_3D('',#{},#{},#{});\n#{}=CARTESIAN_POINT('',({:?},{:?},{:?}));\n#{}=DIRECTION('',({:?},{:?},{:?}));\n#{}=DIRECTION('',({:?},{:?},{:?}));\n",
        id + 1,
        id + 2,
        id + 3,
        id + 1,
        at[0],
        at[1],
        at[2],
        id + 2,
        z[0],
        z[1],
        z[2],
        id + 3,
        x[0],
        x[1],
        x[2]
    )
}

/// The part placed in the assembly at placement `to` by relationship `id`, with the
/// transformation's two placements in the order given.
fn placed_by_relationship(id: u32, a: u32, b: u32) -> String {
    format!(
        "#{id}=(REPRESENTATION_RELATIONSHIP('','',#300,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{}) SHAPE_REPRESENTATION_RELATIONSHIP());\n#{}=ITEM_DEFINED_TRANSFORMATION('','',#{a},#{b});\n",
        id + 1,
        id + 1
    )
}

const Z: [f64; 3] = [0.0, 0.0, 1.0];
const X: [f64; 3] = [1.0, 0.0, 0.0];

#[test]
fn an_assembly_measures_its_parts_where_they_are_placed() {
    // Two uses of the box, side by side: x 0..10 and 30..40.
    let text = assembly(
        &["#410", "#420"],
        &format!(
            "{}{}{}{}",
            axis(410, [0.0; 3], Z, X),
            axis(420, [30.0, 0.0, 0.0], Z, X),
            placed_by_relationship(500, 320, 410),
            placed_by_relationship(510, 320, 420)
        ),
        "",
    );
    let (s, warnings) = size(&text);
    assert_eq!(
        s,
        ModelSize {
            width: 40.0,
            height: 20.0,
            depth: 5.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_part_turned_a_quarter_swaps_its_width_and_height() {
    // Its x axis along the assembly's y.
    let turned = axis(420, [100.0, 0.0, 0.0], Z, [0.0, 1.0, 0.0]);
    let quarter = ModelSize {
        width: 20.0,
        height: 10.0,
        depth: 5.0,
    };
    let text = assembly(
        &["#420"],
        &format!("{turned}{}", placed_by_relationship(510, 320, 420)),
        "",
    );
    assert_eq!(size(&text).0, quarter);
    // Writers that list the placements the other way round: told apart by which
    // representation each placement belongs to.
    let text = assembly(
        &["#420"],
        &format!("{turned}{}", placed_by_relationship(510, 420, 320)),
        "",
    );
    let (s, warnings) = size(&text);
    assert_eq!(s, quarter);
    assert!(warnings.is_empty(), "{warnings:?}");
    // The same with a mapped item.
    let text = assembly(
        &["#600"],
        &turned,
        "#601=REPRESENTATION_MAP(#320,#300);\n#600=MAPPED_ITEM('',#601,#420);\n",
    );
    let (s, warnings) = size(&text);
    assert_eq!(s, quarter);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn nested_assemblies_add_up_their_placements() {
    // The box in a sub-assembly (#700) shifted 50 along x and 7 along y, and the sub-assembly
    // used twice in the top assembly, the second 100 further along y and lying on its back (z
    // down, y flipped): the inner shift must turn with it.
    let text = assembly(
        &["#410", "#420"],
        &format!(
            "{}{}{}{}{}{}",
            axis(410, [0.0; 3], Z, X),
            axis(420, [0.0, 100.0, 0.0], [0.0, 0.0, -1.0], X),
            axis(710, [50.0, 7.0, 0.0], Z, X),
            axis(720, [0.0; 3], Z, X),
            "#700=SHAPE_REPRESENTATION('',(#710,#720),#901);\n#750=(REPRESENTATION_RELATIONSHIP('','',#300,#700) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#751) SHAPE_REPRESENTATION_RELATIONSHIP());\n#751=ITEM_DEFINED_TRANSFORMATION('','',#320,#710);\n",
            "#760=(REPRESENTATION_RELATIONSHIP('','',#700,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#761) SHAPE_REPRESENTATION_RELATIONSHIP());\n#761=ITEM_DEFINED_TRANSFORMATION('','',#720,#410);\n#770=(REPRESENTATION_RELATIONSHIP('','',#700,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#771) SHAPE_REPRESENTATION_RELATIONSHIP());\n#771=ITEM_DEFINED_TRANSFORMATION('','',#720,#420);\n"
        ),
        "",
    );
    let (s, warnings) = size(&text);
    // x 50..60; y 7..27 and, flipped about x, 73..93; z 0..5 and -5..0.
    assert_eq!(
        s,
        ModelSize {
            width: 10.0,
            height: 86.0,
            depth: 10.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn placements_in_a_loop_end_with_a_warning() {
    // The part placed in the assembly and the assembly placed back in the part.
    let text = assembly(
        &["#410"],
        &format!(
            "{}{}#530=(REPRESENTATION_RELATIONSHIP('','',#400,#300) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#531) SHAPE_REPRESENTATION_RELATIONSHIP());\n#531=ITEM_DEFINED_TRANSFORMATION('','',#410,#320);\n",
            axis(410, [5.0, 0.0, 0.0], Z, X),
            placed_by_relationship(500, 320, 410),
        ),
        "",
    );
    let (s, warnings) = size(&text);
    assert!(s.is_usable());
    assert!(warnings.join("\n").contains("in a loop"), "{warnings:?}");
}

#[test]
fn a_circle_turned_by_a_placement_keeps_its_size() {
    // Two radius-10 circles 5 mm apart along z, in a part stood up (its z along x) and turned
    // 45 degrees about that: the circles lie in the y-z plane, exactly 20 across both ways.
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let text = step(
        MM,
        &format!(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=AXIS2_PLACEMENT_3D('',#1,$,$);\n#3=CIRCLE('',#2,10.);\n\
             #4=CARTESIAN_POINT('',(0.,0.,5.));\n#5=AXIS2_PLACEMENT_3D('',#4,$,$);\n#6=CIRCLE('',#5,10.);\n\
             #321=CARTESIAN_POINT('',(0.,0.,0.));\n#320=AXIS2_PLACEMENT_3D('',#321,$,$);\n\
             #901=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#900)) REPRESENTATION_CONTEXT('',''));\n\
             #300=SHAPE_REPRESENTATION('',(#320,#3,#6),#901);\n#400=SHAPE_REPRESENTATION('',(#420),#901);\n{}{}",
            axis(420, [50.0, 0.0, 0.0], X, [0.0, r, r]),
            placed_by_relationship(510, 320, 420)
        ),
    );
    let (s, warnings) = size(&text);
    assert!(
        (s.width - 5.0).abs() < 1e-9 && (s.height - 20.0).abs() < 1e-9 && (s.depth - 20.0).abs() < 1e-9,
        "{s:?}"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_reference_direction_close_to_the_axis_still_gives_the_turn() {
    // Its part square to the axis points along y: the part's x runs along the assembly's y.
    let text = assembly(
        &["#420"],
        &format!(
            "{}{}",
            axis(420, [0.0; 3], Z, [0.0, 0.00001, 1.0]),
            placed_by_relationship(510, 320, 420)
        ),
        "",
    );
    assert_eq!(
        size(&text).0,
        ModelSize {
            width: 20.0,
            height: 10.0,
            depth: 5.0
        }
    );
}

#[test]
fn a_relationship_written_as_one_record_is_read() {
    let single = |id: u32, to: u32| {
        format!(
            "#{id}=REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION('','',#300,#400,#{});\n#{}=ITEM_DEFINED_TRANSFORMATION('','',#320,#{to});\n",
            id + 1,
            id + 1
        )
    };
    let text = assembly(
        &["#410", "#420"],
        &format!(
            "{}{}{}{}",
            axis(410, [0.0; 3], Z, X),
            axis(420, [30.0, 0.0, 0.0], Z, X),
            single(500, 410),
            single(510, 420)
        ),
        "",
    );
    let (s, warnings) = size(&text);
    assert_eq!(s.width, 40.0);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_placement_used_twice_does_not_hide_one_that_failed() {
    // #501 places the part twice; #521 names a placement that does not exist.
    let text = assembly(
        &["#410"],
        &format!(
            "{}{}#510=(REPRESENTATION_RELATIONSHIP('','',#300,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#501) SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #520=(REPRESENTATION_RELATIONSHIP('','',#300,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#521) SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #521=ITEM_DEFINED_TRANSFORMATION('','',#320,#999);\n",
            axis(410, [0.0; 3], Z, X),
            placed_by_relationship(500, 320, 410),
        ),
        "",
    );
    let (_, warnings) = size(&text);
    assert!(
        warnings.join("\n").contains("1 of its placements could not be applied"),
        "{warnings:?}"
    );
}

#[test]
fn geometry_two_representations_share_is_measured_in_both() {
    // A second part representation (#310) lists the same box vertices as #300; each is placed
    // once, at x 0 and 30.
    let vertices: Vec<String> = (1..=8).map(|k| format!("#{}", 2 * k)).collect();
    let text = assembly(
        &["#410", "#420"],
        &format!(
            "{}{}#310=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#311,{}),#901);\n#311=AXIS2_PLACEMENT_3D('',#321,$,$);\n{}\
             #510=(REPRESENTATION_RELATIONSHIP('','',#310,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#511) SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #511=ITEM_DEFINED_TRANSFORMATION('','',#311,#420);\n",
            axis(410, [0.0; 3], Z, X),
            axis(420, [30.0, 0.0, 0.0], Z, X),
            vertices.join(","),
            placed_by_relationship(500, 320, 410),
        ),
        "",
    );
    let (s, warnings) = size(&text);
    assert_eq!(
        s,
        ModelSize {
            width: 40.0,
            height: 20.0,
            depth: 5.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_mapped_item_can_target_a_transformation_operator() {
    // Two uses of the box through operators: one at x 0, one at x 30 turned a quarter about z
    // (axis1 along y) and scaled by 2, so 40 wide and 20 deep from x 30 back to x -10.
    let text = assembly(
        &["#600", "#610"],
        "#601=REPRESENTATION_MAP(#320,#300);\n\
         #600=MAPPED_ITEM('',#601,#602);\n\
         #602=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',$,$,#603,$,$);\n\
         #603=CARTESIAN_POINT('',(0.,0.,0.));\n\
         #610=MAPPED_ITEM('',#601,#612);\n\
         #612=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',#614,$,#613,2.,$);\n\
         #613=CARTESIAN_POINT('',(30.,0.,0.));\n\
         #614=DIRECTION('',(0.,1.,0.));\n",
        "",
    );
    let (s, warnings) = size(&text);
    // x: 0..10 and 30-40..30, so -10..30; y: 0..20 and 0..20; z: 0..5 and 0..10.
    assert_eq!(
        s,
        ModelSize {
            width: 40.0,
            height: 20.0,
            depth: 10.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn an_origin_both_representations_list_does_not_swap_the_placements() {
    // The parent also lists the part's origin #320; the file keeps the standard order. The
    // part's x, y and z go to the assembly's y, z and x: swapped, the turn would go the other
    // way round and give 20 x 5 x 10.
    let turned = axis(420, [100.0, 0.0, 0.0], X, [0.0, 1.0, 0.0]);
    let text = assembly(
        &["#420", "#320"],
        &format!("{turned}{}", placed_by_relationship(510, 320, 420)),
        "",
    );
    let (s, warnings) = size(&text);
    assert_eq!(
        s,
        ModelSize {
            width: 5.0,
            height: 10.0,
            depth: 20.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn an_operator_without_axis2_takes_the_world_y() {
    // axis1 along -x and no axis2: y stays the world's y (a mirror), as the standard's default
    // has it. The box at (0, 30, 0) then spans x -10..0 and y 30..50, next to a copy as drawn.
    let text = assembly(
        &["#600", "#610"],
        "#601=REPRESENTATION_MAP(#320,#300);\n\
         #600=MAPPED_ITEM('',#601,#602);\n\
         #602=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',$,$,#603,$,$);\n\
         #603=CARTESIAN_POINT('',(0.,0.,0.));\n\
         #610=MAPPED_ITEM('',#601,#612);\n\
         #612=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',#614,$,#613,$,$);\n\
         #613=CARTESIAN_POINT('',(0.,30.,0.));\n\
         #614=DIRECTION('',(-1.,0.,0.));\n",
        "",
    );
    let (s, warnings) = size(&text);
    assert_eq!(
        s,
        ModelSize {
            width: 20.0,
            height: 50.0,
            depth: 5.0
        }
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn representations_in_different_contexts_are_not_drawn_together() {
    // #350 is related to the part without a transformation but drawn in another context: it is
    // not moved with the part (to x 30), and its point stays at x 500.
    let text = assembly(
        &["#410"],
        &format!(
            "{}{}",
            axis(410, [30.0, 0.0, 0.0], Z, X),
            placed_by_relationship(500, 320, 410)
        ),
        "#902=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#900)) REPRESENTATION_CONTEXT('',''));\n\
         #360=CARTESIAN_POINT('',(500.,0.,0.));\n#361=VERTEX_POINT('',#360);\n\
         #350=SHAPE_REPRESENTATION('',(#361),#902);\n\
         #355=SHAPE_REPRESENTATION_RELATIONSHIP('','',#350,#300);\n",
    );
    let (s, _) = size(&text);
    assert_eq!(s.width, 470.0, "{s:?}");
}

#[test]
fn a_relationship_whose_transformation_is_missing_warns() {
    let text = assembly(
        &["#410"],
        &format!(
            "{}#510=(REPRESENTATION_RELATIONSHIP('','',#300,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#999) SHAPE_REPRESENTATION_RELATIONSHIP());\n",
            axis(410, [0.0; 3], Z, X),
        ),
        "",
    );
    let (_, warnings) = size(&text);
    assert!(
        warnings.join("\n").contains("1 of its placements could not be applied"),
        "{warnings:?}"
    );
}

#[test]
fn a_loose_point_model_does_not_measure_an_operator_origin() {
    // Loose points 10 x 20 x 5 apart, mapped 100 along each axis by an operator whose origin
    // is a point of its own.
    let text = step(
        MM,
        "#1=CARTESIAN_POINT('',(100.,100.,100.));\n#2=CARTESIAN_POINT('',(110.,120.,105.));\n\
         #321=CARTESIAN_POINT('',(0.,0.,0.));\n#320=AXIS2_PLACEMENT_3D('',#321,$,$);\n\
         #901=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#900)) REPRESENTATION_CONTEXT('',''));\n\
         #300=SHAPE_REPRESENTATION('',(#320,#1,#2),#901);\n#400=SHAPE_REPRESENTATION('',(#600),#901);\n\
         #601=REPRESENTATION_MAP(#320,#300);\n#600=MAPPED_ITEM('',#601,#602);\n\
         #602=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',$,$,#603,$,$);\n\
         #603=CARTESIAN_POINT('',(100.,100.,100.));\n",
    );
    let (s, _) = size(&text);
    assert_eq!(
        s,
        ModelSize {
            width: 10.0,
            height: 20.0,
            depth: 5.0
        }
    );
}

#[test]
fn a_direction_that_is_not_one_fails_its_placement() {
    let text = assembly(
        &["#410"],
        &format!(
            "#410=AXIS2_PLACEMENT_3D('',#411,'up',$);\n#411=CARTESIAN_POINT('',(0.,0.,0.));\n{}",
            placed_by_relationship(500, 320, 410)
        ),
        "",
    );
    let (_, warnings) = size(&text);
    assert!(
        warnings.join("\n").contains("1 of its placements could not be applied"),
        "{warnings:?}"
    );
}

#[test]
fn geometry_shared_by_parts_placed_only_in_a_loop_is_kept_once() {
    // The loop of `placements_in_a_loop_end_with_a_warning`, and a second part (#310) listing
    // the same box vertices placed in the looping assembly too: neither part is reached, so
    // both are measured as drawn, and the eight vertices they share count once, not twice.
    let vertices: Vec<String> = (1..=8).map(|k| format!("#{}", 2 * k)).collect();
    let text = assembly(
        &["#410", "#430"],
        &format!(
            "{}{}{}#530=(REPRESENTATION_RELATIONSHIP('','',#400,#300) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#531) SHAPE_REPRESENTATION_RELATIONSHIP());\n#531=ITEM_DEFINED_TRANSFORMATION('','',#410,#320);\n\
             #310=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#311,{}),#901);\n#311=AXIS2_PLACEMENT_3D('',#321,$,$);\n\
             #540=(REPRESENTATION_RELATIONSHIP('','',#310,#400) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#541) SHAPE_REPRESENTATION_RELATIONSHIP());\n#541=ITEM_DEFINED_TRANSFORMATION('','',#311,#430);\n",
            axis(410, [5.0, 0.0, 0.0], Z, X),
            axis(430, [0.0; 3], Z, X),
            placed_by_relationship(500, 320, 410),
            vertices.join(","),
        ),
        "",
    );
    let (instances, _) = parse(&text);
    let point = |id: u64| -> Option<[f64; 3]> {
        let (name, args) = simple(&instances, id)?;
        (name == "CARTESIAN_POINT").then_some(())?;
        coords(args.get(1)?)
    };
    let pts: Vec<(u64, Geom)> = (1..=8u64)
        .filter_map(|k| point(2 * k - 1).map(|p| (2 * k, Geom::Point(p))))
        .collect();
    assert_eq!(pts.len(), 8);
    let placed = assembly::place(&text, &instances, pts, &point, 1.0, &|_| Some(1.0));
    assert!(placed.cut);
    assert_eq!(placed.points.len(), 8);
}

#[test]
fn a_loose_point_that_is_also_an_operator_origin_is_still_measured() {
    // The representation lists both points; an identity operator takes the first as its
    // origin. It is geometry too, so both are measured.
    let text = step(
        MM,
        "#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=CARTESIAN_POINT('',(10.,20.,5.));\n\
         #901=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#900)) REPRESENTATION_CONTEXT('',''));\n\
         #300=SHAPE_REPRESENTATION('',(#1,#2),#901);\n\
         #602=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',$,$,#1,$,$);\n",
    );
    let (s, _) = size(&text);
    assert_eq!(
        s,
        ModelSize {
            width: 10.0,
            height: 20.0,
            depth: 5.0
        }
    );
}

/// The box placed by an axis placement with axis `z` and no reference direction.
fn placed_on_axis(z: [f64; 3]) -> String {
    assembly(
        &["#410"],
        &format!(
            "#410=AXIS2_PLACEMENT_3D('',#411,#412,$);\n#411=CARTESIAN_POINT('',(0.,0.,0.));\n#412=DIRECTION('',({:?},{:?},{:?}));\n{}",
            z[0],
            z[1],
            z[2],
            placed_by_relationship(500, 320, 410)
        ),
        "",
    )
}

#[test]
fn an_axis_all_but_along_x_still_takes_the_world_x_as_reference() {
    // Without a reference direction the world's x, made square to the axis, is the part's x
    // (here world -z) however little the axis leans off x; its y stays the world's y.
    for tilt in [1e-15, 1e-200] {
        let (s, warnings) = size(&placed_on_axis([1.0, 0.0, tilt]));
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            near(s.width, 5.0) && near(s.height, 20.0) && near(s.depth, 10.0),
            "tilt {tilt}: {s:?}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    // Exactly along x the world's y is the part's x and z its y.
    let (s, _) = size(&placed_on_axis([1.0, 0.0, 0.0]));
    assert_eq!(
        s,
        ModelSize {
            width: 5.0,
            height: 10.0,
            depth: 20.0
        }
    );
}

#[test]
fn an_operator_axis2_all_but_along_its_axes_keeps_y_square() {
    // axis3 and axis1 lie in the world's xy; axis2 leans 1e-17 off them toward world z, so
    // y is world z, and the box's 20 mm stand up whole.
    let text = assembly(
        &["#600"],
        "#601=REPRESENTATION_MAP(#320,#300);\n\
         #600=MAPPED_ITEM('',#601,#602);\n\
         #602=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','','',#604,#605,#603,$,#606);\n\
         #603=CARTESIAN_POINT('',(0.,0.,0.));\n\
         #604=DIRECTION('',(-1.,1.,0.));\n\
         #605=DIRECTION('',(1.,1.,1.E-17));\n\
         #606=DIRECTION('',(1.,1.,0.));\n",
        "",
    );
    let (s, warnings) = size(&text);
    let diagonal = 15.0 / 2f64.sqrt();
    assert!((s.width - diagonal).abs() < 1e-6, "{s:?}");
    assert!((s.height - diagonal).abs() < 1e-6, "{s:?}");
    assert!((s.depth - 20.0).abs() < 1e-6, "{s:?}");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn an_inch_part_placed_in_a_millimetre_assembly_is_converted_before_it_is_placed() {
    // The part, 1 x 2 x 3 inches, is drawn in an inch context and placed twice, at x 0 and 50 mm,
    // in a millimetre assembly: 50 + 25.4 = 75.4 mm across.
    let part = box_vertices([0.0; 3], [1.0, 2.0, 3.0]);
    let vertices: Vec<String> = (1..=8).map(|k| format!("#{}", 2 * k)).collect();
    let text = step(
        MM,
        &format!(
            "{part}#321=CARTESIAN_POINT('',(0.,0.,0.));\n#320=AXIS2_PLACEMENT_3D('',#321,$,$);\n\
             #901=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#900)) REPRESENTATION_CONTEXT('',''));\n\
             #910=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#900);\n\
             #911=DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\n\
             #912=(CONVERSION_BASED_UNIT('INCH',#910)LENGTH_UNIT()NAMED_UNIT(#911));\n\
             #905=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#912)) REPRESENTATION_CONTEXT('',''));\n\
             #300=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#320,{}),#905);\n\
             #400=SHAPE_REPRESENTATION('',(#410,#420),#901);\n{}{}{}{}",
            vertices.join(","),
            axis(410, [0.0; 3], Z, X),
            axis(420, [50.0, 0.0, 0.0], Z, X),
            placed_by_relationship(500, 320, 410),
            placed_by_relationship(510, 320, 420)
        ),
    );
    let (s, warnings) = size(&text);
    let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(
        near(s.width, 75.4) && near(s.height, 50.8) && near(s.depth, 76.2),
        "{s:?}"
    );
    // Each part had its own unit: nothing was guessed.
    assert!(warnings.is_empty(), "{warnings:?}");

    // The same through mapped items: the map's origin is in the part's inches, the target in
    // the assembly's millimetres.
    let mapped = text.replace(
        "#400=SHAPE_REPRESENTATION('',(#410,#420),#901);",
        "#400=SHAPE_REPRESENTATION('',(#600,#610),#901);\n#601=REPRESENTATION_MAP(#320,#300);\n\
         #600=MAPPED_ITEM('',#601,#410);\n#610=MAPPED_ITEM('',#601,#420);",
    );
    let mapped = mapped
        .lines()
        .filter(|l| {
            !l.starts_with("#500=") && !l.starts_with("#510=") && !l.starts_with("#501=") && !l.starts_with("#511=")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (s, warnings) = size(&mapped);
    assert!(
        near(s.width, 75.4) && near(s.height, 50.8) && near(s.depth, 76.2),
        "{s:?}"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}
