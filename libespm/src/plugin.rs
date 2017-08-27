/*
 * This file is part of libespm
 *
 * Copyright (C) 2017 Oliver Hamlet
 *
 * libespm is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * libespm is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with libespm. If not, see <http://www.gnu.org/licenses/>.
 */

use std::borrow::Cow;
use std::io::Cursor;
use std::fs::File;
use std::io;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::str;

use byteorder::{LittleEndian, ReadBytesExt};

use encoding::{Encoding, DecoderTrap};
use encoding::all::WINDOWS_1252;

use nom::ErrorKind;
use nom::IError;
use nom::IResult;

use memmap::Mmap;
use memmap::Protection;

use form_id::FormId;
use game_id::GameId;
use group::Group;
use record::Record;

#[derive(Debug)]
pub enum Error {
    NonUtf8FilePath,
    NonUtf8StringData,
    IoError(io::Error),
    NoFilename,
    ParsingIncomplete,
    ParsingError,
    DecodeError(Cow<'static, str>),
}

impl From<IError> for Error {
    fn from(error: IError) -> Self {
        match error {
            IError::Error(_) => Error::ParsingError,
            _ => Error::ParsingIncomplete,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::IoError(error)
    }
}

impl From<Cow<'static, str>> for Error {
    fn from(error: Cow<'static, str>) -> Self {
        Error::DecodeError(error)
    }
}

#[derive(Clone, Debug, Default)]
struct PluginData {
    header_record: Record,
    form_ids: Vec<FormId>,
}

#[derive(Clone, Debug)]
pub struct Plugin {
    game_id: GameId,
    path: PathBuf,
    data: PluginData,
}

impl Plugin {
    pub fn new(game_id: GameId, filepath: &Path) -> Plugin {
        Plugin {
            game_id: game_id,
            path: filepath.to_path_buf(),
            data: PluginData::default(),
        }
    }

    pub fn parse(&mut self, input: &[u8], load_header_only: bool) -> Result<(), Error> {
        match self.filename() {
            None => Err(Error::NoFilename),
            Some(filename) => {
                self.data = parse_plugin(input, self.game_id, &filename, load_header_only)
                    .to_full_result()?;

                Ok(())
            }
        }
    }

    pub fn parse_file(&mut self, load_header_only: bool) -> Result<(), Error> {
        let f = File::open(self.path.clone())?;

        let mut reader = BufReader::new(f);

        let mut content: Vec<u8> = Vec::new();
        reader.read_to_end(&mut content)?;

        self.parse(&content, load_header_only)
    }

    pub unsafe fn parse_mmapped_file(&mut self, load_header_only: bool) -> Result<(), Error> {
        let mmap_view = Mmap::open_path(self.path.as_path(), Protection::Read)?
            .into_view();

        let mmap_slice = mmap_view.as_slice();

        self.parse(mmap_slice, load_header_only)
    }

    pub fn game_id(&self) -> &GameId {
        &self.game_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn filename(&self) -> Option<String> {
        self.path
            .file_name()
            .and_then(|filename| filename.to_str())
            .map(|filename| filename.to_string())
    }

    pub fn masters(&self) -> Result<Vec<String>, Error> {
        masters(&self.data.header_record)
    }

    pub fn is_master_file(&self) -> bool {
        if self.game_id != GameId::Morrowind {
            self.data.header_record.header.flags & 0x1 != 0
        } else {
            match self.path.extension() {
                Some(x) if x == "esm" => true,
                Some(x) if x == "ghost" => {
                    match self.path.file_stem().and_then(
                        |file_stem| file_stem.to_str(),
                    ) {
                        Some(file_stem) => file_stem.ends_with(".esm"),
                        None => false,
                    }
                }
                _ => false,
            }
        }
    }

    pub fn is_valid(game_id: GameId, filepath: &Path, load_header_only: bool) -> bool {
        let mut plugin = Plugin::new(game_id, &filepath.to_path_buf());

        match plugin.parse_file(load_header_only) {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    pub fn description(&self) -> Result<Option<String>, Error> {
        let (target_subrecord_type, description_offset) = if self.game_id == GameId::Morrowind {
            ("HEDR", 40)
        } else {
            ("SNAM", 0)
        };

        for subrecord in &self.data.header_record.subrecords {
            if subrecord.subrecord_type == target_subrecord_type {
                let data = &subrecord.data[description_offset..(subrecord.data.len() - 1)];

                return WINDOWS_1252
                    .decode(data, DecoderTrap::Strict)
                    .map(Option::Some)
                    .map_err(Error::DecodeError);
            }
        }

        Ok(Option::None)
    }

    pub fn record_and_group_count(&self) -> Option<u32> {
        let count_offset = if self.game_id == GameId::Morrowind {
            296
        } else {
            4
        };

        for subrecord in &self.data.header_record.subrecords {
            if subrecord.subrecord_type == "HEDR" {
                let data = &subrecord.data[count_offset..count_offset + 4];
                let mut cursor = Cursor::new(data);
                return cursor.read_u32::<LittleEndian>().ok();
            }
        }

        Option::None
    }

    pub fn form_ids(&self) -> &Vec<FormId> {
        &self.data.form_ids
    }
}

fn masters(header_record: &Record) -> Result<Vec<String>, Error> {
    header_record
        .subrecords
        .iter()
        .filter(|s| s.subrecord_type == "MAST")
        .map(|s| &s.data[0..(s.data.len() - 1)])
        .map(|d| {
            WINDOWS_1252.decode(d, DecoderTrap::Strict).map_err(
                Error::DecodeError,
            )
        })
        .collect::<Result<Vec<String>, Error>>()
}

fn parse_form_ids<'a>(
    input: &'a [u8],
    game_id: GameId,
    filename: &str,
    header_record: &Record,
) -> IResult<&'a [u8], Vec<FormId>> {
    let masters = match masters(header_record) {
        Ok(x) => x,
        Err(_) => return IResult::Error(ErrorKind::Custom(1)),
    };

    if game_id == GameId::Morrowind {
        let (input1, record_form_ids) =
            try_parse!(input, many0!(apply!(Record::parse_form_id, game_id)));

        let form_ids: Vec<FormId> = record_form_ids
            .into_iter()
            .map(|form_id| FormId::new(filename, &masters, form_id))
            .collect();

        IResult::Done(input1, form_ids)
    } else {
        let (input1, groups) = try_parse!(input, many0!(apply!(Group::new, game_id)));

        let mut form_ids: Vec<FormId> = Vec::new();
        for group in groups {
            form_ids.extend(group.form_ids.into_iter().map(|form_id| {
                FormId::new(filename, &masters, form_id)
            }));
        }

        IResult::Done(input1, form_ids)
    }
}

fn parse_plugin<'a>(
    input: &'a [u8],
    game_id: GameId,
    filename: &str,
    load_header_only: bool,
) -> IResult<&'a [u8], PluginData> {
    let (input1, header_record) = try_parse!(input, apply!(Record::parse, game_id, false));

    if load_header_only {
        return IResult::Done(
            input1,
            PluginData {
                header_record: header_record,
                form_ids: Vec::new(),
            },
        );
    }

    let (input2, form_ids) = try_parse!(
        input1,
        apply!(parse_form_ids, game_id, filename, &header_record)
    );

    IResult::Done(
        input2,
        PluginData {
            header_record: header_record,
            form_ids: form_ids,
        },
    )
}