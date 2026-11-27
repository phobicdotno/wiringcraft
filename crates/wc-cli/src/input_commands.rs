/// A command line for every command that reads a named input file. `FIFO` and `FILE` stand
/// for its input files, in the order the command reads them, and `LIB` for a library folder.
/// Included by the unit test that checks `Cmd::inputs_mut` knows every one of them, and by
/// the test that runs each one with a FIFO for `FIFO` while the project lock is held (#658),
/// so both go through the same commands.
const INPUT_COMMANDS: &[&[&str]] = &[
    &["inspect", "FIFO"],
    &["logo", "set", "FIFO"],
    &["rules", "set", "FIFO"],
    &["report-layout", "set", "terminals", "FIFO"],
    &[
        "import",
        "symbol-dxf",
        "FIFO",
        "--id",
        "t.dxf",
        "--name",
        "A",
        "--lib",
        "LIB",
    ],
    &["import", "footprint-dxf", "FIFO", "--part", "t.part", "--lib", "LIB"],
    &["import", "model-step", "FIFO", "--part", "t.part", "--lib", "LIB"],
    &[
        "import",
        "symbol-svg",
        "FIFO",
        "--id",
        "t.svg",
        "--name",
        "A",
        "--lib",
        "LIB",
    ],
    &["import", "plc-csv", "FIFO"],
    &["import", "wago", "FIFO"],
    &["import", "symbol-kicad", "FIFO", "--lib", "LIB"],
    &["import", "parts-bmecat", "FIFO", "--lib", "LIB"],
    &["import", "parts-edz", "FIFO", "--lib", "LIB"],
    &["import", "parts-edz", "FILE", "--lib", "LIB", "--symbol-map", "FIFO"],
    &["import", "symbol-qet", "FIFO", "FILE", "FIFO", "--lib", "LIB"],
    &["import", "parts-csv", "FIFO", "--lib", "LIB"],
    &["import", "archive", "FIFO"],
    &["import", "page-dwg", "FIFO", "--page", "p1"],
    &["import", "pdf", "FIFO"],
    &["import", "eplan", "FIFO"],
    &["import", "swe", "FIFO"],
    &["import", "swe", "FILE", "--symbol-map", "FIFO"],
    &["import", "swe-library", "FIFO"],
    &["import", "aml", "FIFO"],
    &["import", "page-dxf", "FIFO", "--page", "p1"],
];

/// The table above is well formed: the lock test (#658) only proves something for a command
/// that reads at least one FIFO, and it fills in `FILE` and `LIB` by position.
#[cfg(test)]
mod input_command_table {
    use super::INPUT_COMMANDS;

    /// The words the lock test replaces with a path.
    const PLACEHOLDERS: &[&str] = &["FIFO", "FILE", "LIB"];

    #[test]
    fn every_command_reads_at_least_one_fifo() {
        for args in INPUT_COMMANDS {
            assert!(args.contains(&"FIFO"), "{args:?} reads no FIFO");
        }
    }

    #[test]
    fn every_command_starts_with_a_command_word() {
        for args in INPUT_COMMANDS {
            let Some(first) = args.first() else {
                unreachable!("an empty command line is in the table");
            };
            assert!(!PLACEHOLDERS.contains(first), "{args:?}");
            assert!(!first.starts_with('-'), "{args:?}");
        }
    }

    #[test]
    fn no_command_line_is_listed_twice() {
        for (n, args) in INPUT_COMMANDS.iter().enumerate() {
            let twice = INPUT_COMMANDS.iter().skip(n + 1).any(|other| other == args);
            assert!(!twice, "{args:?} is listed twice");
        }
    }

    #[test]
    fn a_library_folder_follows_lib_and_only_lib() {
        for args in INPUT_COMMANDS {
            for (k, arg) in args.iter().enumerate() {
                let before = k.checked_sub(1).and_then(|b| args.get(b));
                if *arg == "LIB" {
                    assert_eq!(before, Some(&"--lib"), "{args:?}");
                }
                if *arg == "--lib" {
                    assert_eq!(args.get(k + 1), Some(&"LIB"), "{args:?}");
                }
            }
        }
    }

    #[test]
    fn an_input_file_is_never_an_option_name_value() {
        // `--id`, `--name`, `--part` and `--page` take a plain value, never one of the files.
        for args in INPUT_COMMANDS {
            for pair in args.windows(2) {
                let [option, value] = pair else {
                    unreachable!("windows(2) gives pairs");
                };
                if ["--id", "--name", "--part", "--page"].contains(option) {
                    assert!(!PLACEHOLDERS.contains(value), "{args:?}");
                }
            }
        }
    }

    #[test]
    fn the_project_is_left_to_the_caller() {
        // The lock test points every command at its project with `-C`.
        for args in INPUT_COMMANDS {
            assert!(!args.iter().any(|a| ["-C", "--project"].contains(a)), "{args:?}");
        }
    }

    #[test]
    fn a_symbol_map_is_always_a_fifo_next_to_a_regular_file() {
        // Where a symbol map is the input under test, the main input is a regular file.
        let mut seen = 0;
        for args in INPUT_COMMANDS {
            if let Some(k) = args.iter().position(|a| *a == "--symbol-map") {
                assert_eq!(args.get(k + 1), Some(&"FIFO"), "{args:?}");
                assert!(args.contains(&"FILE"), "{args:?}");
                seen += 1;
            }
        }
        assert_eq!(seen, 2);
    }
}
