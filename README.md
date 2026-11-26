ISPFLite is a local editor that works like the IBM ISPF editor. It is written in Rust from the behavior of [SPFLite](https://www.spflite.com/), with help from Grok, and it is aimed at reading and editing COBOL on a PC.

License: GPL-3.0-or-later.

## Major features

- ISPF edit screen with sequence numbers, a prefix area, data columns, and a primary command line.
- COBOL coloring, and an outline of divisions, sections, and paragraphs. A click in the outline shows that part of the file.
- Hold Ctrl to underline a variable or paragraph defined in the same file. Ctrl+click, or right-click and choose Go to definition, jumps there. Ctrl+Left and Ctrl+Right move back and forward. COBOL keywords are not jump targets.
- FIND, CHANGE, and EXCLUDE take the ISPF parameters in any order: NEXT, ALL, FIRST, LAST, PREV, CHARS, PREFIX, SUFFIX, WORD, X, NX, columns, and a pair of labels. FIND ALL also brings back excluded lines that contain a match.
- Line commands for insert, delete, repeat, copy, move, exclude, shift, overlay, and case change, including their block forms.
- Labels, bookmarks, LOCATE, RESET, SORT, RENUM for standard and COBOL sequence numbers, undo, and redo.
- Several open files. Tabs can be reordered, closed, saved, or moved into a new window, and recent files are remembered.
- Open and save UTF-8 and Windows ANSI (code page 1252) files.
