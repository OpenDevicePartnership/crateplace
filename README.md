# CratePlace
This tool controls the placement of crates in flash memory for embedded Rust projects using `memory.x`.
Crateplace generates `memory.x` based on a new config file called `Memory.toml` this means the original `memory.x` is no longer necessary.
Crateplace also allows manually specifying where some of the symbols end up in memory.

In `Memory.toml` sections of memory can be defined that will house the crates assigned to them.
The default ram and flash sections are mandatory but after that additional sections can be added.
When specifying what section crates should go the tool will automatically place all its dependencies in the specified section unless disabled.
In order to resolve conflicts between shared dependencies of two crates sections are assigned a priority.
If a conflict occurs the section with the highest priority will win (the lowest number has the highest priority).

The tool can be accessed as a library or as a binary.
The binary can generate the `memory.x` linkerscript manually and can show a tree view of the assigned crates and the assigned sections.

Crateplace allows the placing of `text`, `rodata` and `data.rel` sections.
`data` sections are ignored as they should be copied to ram on init and crateplace currently has no mechanism to account for this.

## Setup
The commandline tool can be installed by running the following command:
```
cargo install --path <crate root>
```
This will install the tool.
The tool can be called with:
```
cargo crateplace --help
```

## Usage
To setup a crate the `init` command can be used in the crate root.
This will make a default `Memory.toml` and `build.rs`.
Update the `Memory.toml` to reflect the correct size and offset of the ram and the flash.
Ensure to delete or rename the existing `memory.x` as it will conflict with the output of crateplace.

Alternatively the files can be made manually. The default `Memory.toml` looks like this:
```
ram = { origin = \"0x20000000\", length = \"128K\" }

[sections]
flash = { origin = \"0x00000000\", length = \"1M\", priority = 1 }

[crates]
```
And the default `build.rs` looks like this:
```
fn main() {
    if let Err(err) = crateplace::CratePlacer::new().buildscript() {
        crateplace::report(&err);
        std::process::exit(1);
    }
}
```
The buildscript now automatically generates the the `memory.x` file and places it in the crates output directory.
The file will be automatically included during the build process.


## Configuration
The `Memory.toml` contains the following configuration:
### Ram
This defines the position of ram in memory.
It is defined by a start offset and length just like in `memory.x`.
```
[sections]
ram = { origin = \"0x20000000\", length = \"128K\" }
```
### Sections
The definition of flash sections.
Symbols and crates can be assigned to these sections.
They are defined by an `origin` and `length` just like ram.
Additionally they must be assigned a `piority`.
The lowest priority will be assigned when a dependency is shared by two assigned crates.
Then there is the `default` option which is where all unspecified crates will end up if a section is marked with `default=true`.
```
[crates]
flash = { origin = "0x00000000", length = "512K", priority = 1 }
second_flash = { origin = "512K", length = "512K", priority = 2, default = true}
```
### Crates
The crates section is where crates are assigned to sections.
Crates are defined like this:
```
[crates]
<crate-name> = { section = "second_flash", include-dependencies = true }
```
`section` specifies where the crate ends up.
`include-dependencies` means all dependencies are assigned to the same section as this one.
If left out `include-dependencies` is true by default.
### Symbols
The final section allows directly specifying the placement of symbols.
```
[symbols]
"*<symbol-name>*" = { section = "flash", rodata = false, datarel = true, text = true}
```
The symbol name is placed on the left hand of the equals sign.
This is passed verbatim into the linkerscript and is not checked as crateplace has no means of validating the input.
Glob patterns can be used so any symbol starting with `library-name` can be caught using `library-name*`.
These patterns will override the crates section and will include symbols linked in from multiple languages.
The section is designated just like crates with `section`.
By default crateplace will include the `text`, `rodata` and the `data.rel` sections.
These can be disabled induvidually with the `rodata`, `datarel` and the `text` booleans per symbol.
