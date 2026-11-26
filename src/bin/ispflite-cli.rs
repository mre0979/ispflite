// SPDX-License-Identifier: GPL-3.0-or-later
//! Line-oriented front end. The window is `ispflite`.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ispflite::{execute, help_text, Session};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut session = Session::new();
    if let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" || arg == "/?" {
            println!("{}", help_text());
            println!("\nUsage: ispflite-cli [file]");
            return ExitCode::SUCCESS;
        }
        if let Err(err) = session.open_path(std::path::Path::new(&arg)) {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    }

    println!("{}", help_text());
    println!();
    render(&session);
    let stdin = io::stdin();
    let mut locked = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        print!("Command> ");
        let _ = io::stdout().flush();
        let read = locked.read_line(&mut line);
        match read {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
        }
        let text = line.trim();
        if text.eq_ignore_ascii_case("QUIT") || text.eq_ignore_ascii_case("EXIT") {
            break;
        }
        if text.is_empty() {
            render(&session);
            continue;
        }
        match execute(&mut session, text) {
            Ok(message) => {
                if !message.is_empty() {
                    println!("{message}");
                }
            }
            Err(err) => println!("{err}"),
        }
        println!();
        render(&session);
    }
    ExitCode::SUCCESS
}

fn render(session: &Session) {
    let tab = session.current();
    let doc = &tab.doc;
    let dirty = if doc.is_modified() { "*" } else { "" };
    let (cursor_line, cursor_col) = doc.cursor();
    let where_file = tab
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "new file".to_string());
    println!(
        "---- {}{dirty}  {where_file}  {} data  cursor {} col {cursor_col} ----",
        tab.title,
        doc.last_real(),
        doc.line(cursor_line).map(|line| line.seq).unwrap_or(0)
    );
    let rows = doc.visible_rows();
    let last = doc.last_line();
    let focus = rows.len() > 80;
    let mut hiding = false;
    for row in &rows {
        let near = row.line + 15 >= cursor_line && row.line <= cursor_line + 25;
        let edge = row.line == 1 || row.line == last;
        if focus && !near && !edge {
            if !hiding {
                println!("  ...");
                hiding = true;
            }
            continue;
        }
        hiding = false;
        let mark = if row.line == cursor_line { '>' } else { ' ' };
        println!("{mark}{} {}", row.prefix, row.text);
    }
}
