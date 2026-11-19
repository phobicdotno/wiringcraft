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
