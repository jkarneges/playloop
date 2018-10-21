extern crate lewton;
extern crate sdl2;

use std::cmp;
use std::mem;
use std::ptr;
use std::slice;
use std::cell::Cell;
use std::error::Error;
use std::fs::File;
use std::process;
use std::time::Duration;
use std::thread::sleep;
use std::io::{Read, Seek};
use lewton::VorbisError;
use lewton::inside_ogg::OggStreamReader;
use sdl2::audio::{AudioCallback, AudioFormat, AudioSpec, AudioSpecDesired, AudioStatus, AudioCVT};

// some arbitrary number
const PRODUCER_READ_MAX: usize = 4096;

#[derive(Clone, Copy, Debug)]
struct VorbisPos {
	granule: usize,
	offset: usize,
}

struct Player<T: Read + Seek> {
	// sample positions
	pub loop_start: Option<usize>,
	pub loop_end: Option<usize>,

	pub num_channels: u32,
	pub sample_rate: u32,

	reader: OggStreamReader<T>,
	vpos: VorbisPos,
	next_vpos: Option<VorbisPos>,
	buf: Vec<i16>,
	buf_pos: usize,
	abs_pos: usize,
	vstart: Option<VorbisPos>,
	loop_vstart: Option<VorbisPos>,
	expect_granule: Option<usize>,
	skip: usize,
}

impl<T: Read + Seek> Player<T> {
	pub fn new(rdr: T) -> Result<Player<T>, Box<Error>> {
		let reader = OggStreamReader::new(rdr)?;

		let mut loop_start = None;
		let mut loop_end = None;

		for c in &reader.comment_hdr.comment_list {
			if c.0 == "LOOPSTART" {
				let x = c.1.parse::<usize>()?;
				loop_start = Some(x);
			} else if c.0 == "LOOPEND" {
				let x = c.1.parse::<usize>()?;
				loop_end = Some(x);
			}
		}

		Ok(Player {
			loop_start,
			loop_end,
			num_channels: reader.ident_hdr.audio_channels as u32,
			sample_rate: reader.ident_hdr.audio_sample_rate as u32,
			reader,
			vpos: VorbisPos { granule: 0, offset: 0 },
			next_vpos: None,
			buf: Vec::new(),
			buf_pos: 0,
			abs_pos: 0,
			vstart: None,
			loop_vstart: None,
			expect_granule: None,
			skip: 0,
		})
	}

	pub fn read(&mut self, out: &mut [i16]) -> Result<usize, Box<Error>> {
		let num_channels = self.num_channels as usize;

		assert!(out.len() >= num_channels,
			"out not large enough to hold at least 1 sample");

		let mut out_pos = 0;

		while out_pos == 0 {
			while self.buf_pos == self.buf.len() {
				let size = self.add_to_buf()?;
				if size == 0 {
					println!("Done");
					process::exit(0);
				}
			}

			if self.vstart.is_none() && self.abs_pos == 0 {
				// note the start of playback
				self.vstart = Some(self.vpos);
			}

			while self.buf_pos + num_channels <= self.buf.len() {
				if let Some(expect_granule) = self.expect_granule {
					assert!(self.vpos.granule <= expect_granule,
						"ahead of expected granule");

					// if seek went to earlier granule, skip until we get there
					if self.vpos.granule != expect_granule {
						self.buf_pos += num_channels;
						continue;
					}

					self.expect_granule = None;
				}

				if self.skip > 0 {
					self.skip -= 1;
					self.buf_pos += num_channels;
					continue;
				}

				let buf_csample_pos = self.buf_pos / num_channels;

				//println!("{} {} {} {}", self.abs_pos, self.vpos.granule, self.vpos.offset, buf_csample_pos);

				if let Some(loop_start) = self.loop_start {
					if self.loop_vstart.is_none() && self.abs_pos == loop_start {
						let vpos = VorbisPos {
							granule: self.vpos.granule,
							offset: self.vpos.offset + buf_csample_pos,
						};
						self.loop_vstart = Some(vpos);
						println!("Found start: granule={} offset={}", vpos.granule, vpos.offset);
					}
				}

				if let Some(loop_end) = self.loop_end {
					if self.abs_pos == loop_end {
						break;
					}
				}

				if out_pos + num_channels > out.len() {
					break;
				}

				for k in 0..num_channels {
					out[out_pos + k] = self.buf[self.buf_pos + k];
				}

				self.buf_pos += num_channels;
				out_pos += num_channels;

				self.abs_pos += 1;
			}

			if let Some(loop_end) = self.loop_end {
				if self.abs_pos == loop_end {
					if let Some(loop_vstart) = self.loop_vstart {
						// if we have vstart then we have start
						println!("Looping");
						let loop_start = self.loop_start.unwrap();
						self.seek(loop_vstart, loop_start)?;
						break;
					} else {
						println!("Unknown start position, can't loop");
					}
				}
			}
		}

		Ok(out_pos)
	}

	// return number of samples added, or zero if end of stream
	fn add_to_buf(&mut self) -> Result<usize, VorbisError> {
		assert_eq!(self.buf_pos, self.buf.len(),
			"add_to_buf called when buf still has data");

		let num_channels = self.num_channels as usize;

		while self.buf_pos == self.buf.len() {
			if let Some(ref next_vpos) = self.next_vpos {
				self.vpos = *next_vpos;
			}

			let samples = self.reader.read_dec_packet_itl()?;

			let samples = match samples {
				Some(samples) => samples,
				None => {
					self.next_vpos = None;

					// end of stream
					return Ok(0);
				},
			};

			assert_eq!(samples.len() % num_channels, 0,
				"read sample count not aligned with channel count");

			// unwrap here because value should exist after a read
			let granule = self.reader.get_last_absgp().unwrap() as usize;

			let next_offset = if granule == self.vpos.granule {
				self.vpos.offset + (samples.len() / num_channels)
			} else {
				0
			};

			self.next_vpos = Some(VorbisPos {
				granule: granule,
				offset: next_offset,
			});

			if samples.len() == 0 {
				// sometimes the decoder returns 0 samples. just read again
				continue;
			}

			// safe because 0
			unsafe { self.buf.set_len(0); }

			self.buf.reserve(samples.len());

			assert!(samples.len() <= self.buf.capacity());

			// safe because we have at least this much capacity
			unsafe { self.buf.set_len(samples.len()); }

			self.buf.copy_from_slice(&samples);

			self.buf_pos = 0;
		}

		Ok(self.buf.len())
	}

	fn seek(&mut self, pos: VorbisPos, abs_pos: usize) -> Result<(), VorbisError> {
		self.reader.seek_absgp_pg(pos.granule as u64)?;
		self.vpos = pos;
		self.next_vpos = None;
		self.expect_granule = Some(pos.granule);
		self.skip = pos.offset;
		self.abs_pos = abs_pos;
		self.buf_pos = self.buf.len();

		Ok(())
	}
}

// wrap AudioCVT and mark it as safe to pass across threads
struct AudioCVTWrapper {
	ac: AudioCVT,
}

// in theory this should be safe since AudioCVT implements Copy (meaning any
//   internal pointers don't require maintenance) and we never use it from
//   multiple threads at the same time
unsafe impl Send for AudioCVTWrapper {}

struct Producer {
	player: Player<File>,
	acw: AudioCVTWrapper,
	buf: Cell<Vec<u8>>,
}

impl Producer {
	fn new(player: Player<File>, spec: AudioSpec) -> Producer {
		let format;
		if cfg!(target_endian = "big") {
			format = AudioFormat::S16MSB;
		} else {
			format = AudioFormat::S16LSB;
		}

		let ac = AudioCVT::new(
			format,
			player.num_channels as u8,
			player.sample_rate as i32,
			spec.format,
			spec.channels,
			spec.freq
		).expect("failed to create audio converter");

		Producer {
			player,
			acw: AudioCVTWrapper {ac},
			buf: Cell::new(Vec::new()),
		}
	}
}

#[allow(mutable_transmutes)]
fn as_mut_i16slice<T>(s: &mut [T]) -> &mut [i16] {
	let len = s.len() * mem::size_of::<T>();

	if len & 1 != 0 {
		panic!("as_i16slice: input size not multiple of 2");
	}

	unsafe {
		let ns = slice::from_raw_parts(s.as_ptr() as *mut i16, len);
		mem::transmute(ns)
	}
}

impl AudioCallback for Producer {
	type Channel = i16;

	fn callback(&mut self, out: &mut [i16]) {
		let mut out_pos = 0;

		while out_pos < out.len() {
			let mut buf = self.buf.take();

			if buf.len() == 0 {
				let read_max = PRODUCER_READ_MAX * 2 * (self.player.num_channels as usize);

				// safe because 0
				unsafe { buf.set_len(0); }

				buf.reserve(self.acw.ac.capacity(read_max));

				assert!(read_max <= buf.capacity());

				// safe because we have at least this much capacity
				unsafe { buf.set_len(read_max); }

				let size_samples = self.player.read(as_mut_i16slice(&mut buf))
					.expect("failed reading vorbis data");

				// size is number of 16-bit samples; convert to bytes
				let read_actual = size_samples * 2;

				assert!(read_actual <= buf.len());

				// safe because read_actual is <= current len
				unsafe { buf.set_len(read_actual); }

				buf = self.acw.ac.convert(buf);
			}

			let buf_samples = buf.len() / 2;

			let copy_samples = cmp::min(out.len() - out_pos, buf_samples);
			let copy_bytes = copy_samples * 2;

			out[out_pos..(out_pos+copy_samples)].copy_from_slice(
				&as_mut_i16slice(&mut buf)[..copy_samples]);
			out_pos += copy_samples;

			let remaining_bytes = if buf.len() > copy_bytes {
				buf.len() - copy_bytes
			} else {
				0
			};

			unsafe {
				let p = buf.as_mut_ptr();
				ptr::copy(p.offset(copy_bytes as isize), p, remaining_bytes);
				buf.set_len(remaining_bytes);
			}

			self.buf.set(buf);
		}
	}
}

pub fn run(file_name: &str) -> Result<(), Box<Error>> {
	let f = File::open(file_name)?;

	let player = Player::new(f)?;

	match player.loop_start {
		Some(loop_start) => {
			match player.loop_end {
				Some(loop_end) => {
					println!("Loop: start={} end={}", loop_start, loop_end);
				},
				None => {
					println!("LOOP_START set but not LOOP_END");
				}
			}
		},
		None => {
			println!("No loop information");
		},
	}

	let sdl_context = sdl2::init()?;
	let audio = sdl_context.audio()?;

	let spec = AudioSpecDesired {
		freq: Some(44_100),
		channels: Some(2),
		samples: None,
	};

	let device = audio.open_playback(None, &spec, |spec| {
		// since we can't return errors from this callback nor the Producer
		//   callback, any errors from here on out, such as vorbis decoding
		//   or audio conversion errors, simply panic

		// TODO: see if there isn't a better way to handle errors, such as
		//   stopping the playback (possibly asynchronously) while returning
		//   zeros to the audio callback

		Producer::new(player, spec)
	})?;

	println!("Playing...");

	device.resume();

	while device.status() == AudioStatus::Playing {
		sleep(Duration::from_millis(100));
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Cursor;
	use std::io::Write;

	// return raw audio and channel count
	fn get_raw(file_data: &[u8]) -> (Vec<i16>, usize) {
		let mut out = Vec::new();

		let c = Cursor::new(file_data);
		let mut reader = OggStreamReader::new(c).unwrap();

		loop {
			let samples = reader.read_dec_packet_itl().unwrap();

			let mut samples = match samples {
				Some(samples) => samples,
				None => {
					break;
				},
			};

			out.append(&mut samples);
		}

		(out, reader.ident_hdr.audio_channels as usize)
	}

	fn write_i16(file: &mut File, x: i16) {
		let u = x as u16;
		let mut buf: [u8; 2] = [0; 2];
		buf[0] = (u >> 8) as u8;
		buf[1] = (u & 0xff) as u8;
		file.write(&buf).unwrap();
	}

	fn play_compare(file_data: &[u8], loop_start: usize, loop_end: usize, loop_count: usize, raw: &[i16], write_files: bool) {
		let c = Cursor::new(file_data);
		let mut player = Player::new(c).unwrap();

		player.loop_start = Some(loop_start);
		player.loop_end = Some(loop_end);

		println!("Loop: start={} end={}", loop_start, loop_end);

		let loop_len = loop_end - loop_start;
		let total_csamples = loop_start + (loop_len * loop_count);

		let num_channels = player.num_channels as usize;

		let mut a: Vec<i16> = Vec::new();
		a.resize(num_channels, 0);

		let mut b: Vec<i16> = Vec::new();
		b.resize(num_channels, 0);

		let mut rawf = None;
		let mut decf = None;

		if write_files {
			rawf = Some(File::create("looped_raw.raw").unwrap());
			decf = Some(File::create("looped_dec.raw").unwrap());
		}

		let mut i = 0;
		while i < total_csamples {
			const BUF_SIZE: usize = 4096;
			let mut dec_buf: [i16; BUF_SIZE] = [0; BUF_SIZE];

			let actual = player.read(&mut dec_buf).unwrap();

			assert_eq!(actual % num_channels, 0);

			let dec_buf = &dec_buf[..actual];

			let dec_csamples = dec_buf.len() / num_channels;

			let mut k = 0;
			while k < dec_csamples && i < total_csamples {
				for j in 0..num_channels {
					let raw_pos;
					if i < loop_start {
						raw_pos = i;
					} else {
						raw_pos = ((i - loop_start) % loop_len) + loop_start;
					}

					a[j] = raw[(raw_pos * num_channels) + j];
					b[j] = dec_buf[(k * num_channels) + j];
				}

				if write_files {
					for j in 0..num_channels {
						if let Some(ref mut f) = rawf {
							write_i16(f, a[j]);
						}
						if let Some(ref mut f) = decf {
							write_i16(f, b[j]);
						}
					}
				} else {
					assert_eq!(b, a, "failed at {}", i);
				}

				k += 1;
				i += 1;
			}
		}

		println!("stopping at: {}", i);
	}

	#[test]
	fn test_loop() {
		let file_data = include_bytes!("testaudio.ogg");

		println!("decoding");

		let (raw, num_channels) = get_raw(file_data);

		println!("playing");

		// test playback of testaudio.ogg using various loop starts

		// NOTE: sample positions within the first playable granule (5376 for
		//   testaudio.ogg) won't work. this may be a limitation in lewton's
		//   seeking. start positions within the second granule (9920, e.g.
		//   sample 5504) will work though
		let lstarts = [5504, 12345, 80123, 123456];

		for lstart in lstarts.iter() {
			assert!((*lstart * num_channels) < raw.len());
			assert_eq!(raw.len() % num_channels, 0);

			let lend = raw.len() / num_channels;

			play_compare(file_data, *lstart, lend, 3, &raw, false);
		}
	}
}
