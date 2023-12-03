#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(stable_features, unknown_lints, async_fn_in_trait)]


// extern crate alloc;
use bbqueue::{Consumer, Producer};
use core::mem::transmute;
use cyw43::Control;
use defmt::*;
use embassy_time::{Timer};
use rmp3::{RawDecoder, Sample, MAX_SAMPLES_PER_FRAME};
use {defmt_rtt as _, panic_probe as _};
const BUFFER_SIZE: usize = 10240;


// DECODE QUEUE FUNCTION
fn enqueue_frame_size(producer: &mut Producer<'static, BUFFER_SIZE>, size: usize) {
    if let Ok(mut grant_w) = producer.grant_exact(4){
        let frame_size: [u8; 4] = unsafe { transmute([size]) };
        grant_w.buf().copy_from_slice(&frame_size);
        grant_w.commit(4);
    }
}

fn enqueue_frame(
    producer: &mut Producer<'static, BUFFER_SIZE>,
    size: usize,
    buf: [Sample; MAX_SAMPLES_PER_FRAME],
) {
    let dest: [u8; MAX_SAMPLES_PER_FRAME * 2] = unsafe { transmute(buf) };
    if let Ok(mut grant_w) = producer.grant_exact(size) {
        grant_w.buf().copy_from_slice(&dest[..size]);
        grant_w.commit(size);
    }
}

fn decode_queue(producer: &mut Producer<'static, BUFFER_SIZE>, src_buf: &[u8]) -> Result<usize, u8> {
    let mut dest = [Sample::default(); MAX_SAMPLES_PER_FRAME];
    let mut decoder = RawDecoder::new();
    match decoder.next(src_buf, &mut dest) {
        Some((_frame, size)) => {
            enqueue_frame_size(producer, size);
            enqueue_frame(producer, size, dest);
            Ok(size)
        }
        None => {
            warn!("Error decoding frame ln:102");
            Err(0)
        }
    }
}
#[embassy_executor::task]
pub async fn decode_task(
    mut consumer: Consumer<'static, BUFFER_SIZE>,
    mut producer: Producer<'static, BUFFER_SIZE>,
)->! {
    info!("STARTING DECODE_TASK");
    loop {
        Timer::after_micros(0).await;
        if let Ok(grant_r) = consumer.read(){
            match decode_queue(&mut producer, &grant_r) {
                Ok(size) => grant_r.release(size),
                Err(_) => {
                    warn!("decode_task had an error!");
                    grant_r.release(0);
                }
            }
        }
    }
}


//PLAY QUEUE FUNCTIONS
fn dequeue_frame_size(consumer: &mut Consumer<'static, BUFFER_SIZE>) -> usize {
    if let Ok(grant_r) = consumer.read(){
        let mut header = [0u8; 4];
        header.copy_from_slice(&grant_r[..4]);
        let header: [usize; 1] = unsafe { transmute(header) };
        header[0]
    }
    else { 0 }
}

pub fn enqueue_bytes(producer: &mut Producer<'static, BUFFER_SIZE>,buf: &[u8;4096], size:usize){
    if let Ok(mut grant_w) = producer.grant_exact(size){
        grant_w.buf().copy_from_slice(&buf[..size]);
        grant_w.commit(size);
    }
}

#[embassy_executor::task]
pub async fn play_task(/* mut control: Control<'static>,*/ mut consumer: Consumer<'static, BUFFER_SIZE>)->! {
    info!("STARTING PLAY_TASK");
    control.gpio_set(0, false).await;
    let mut led_on: bool = false;
    //check queue status first
    loop{
        Timer::after_micros(1).await;
        let frame_size: usize = dequeue_frame_size(&mut consumer); // should branch on 0
        if let Ok(grant_r) = consumer.read(){
            info!("SETTING LED ON");
            led_on = true;
            for sample in grant_r.chunks_exact(2) {
                let sample: [Sample;1] = unsafe{transmute([sample[0],sample[1]])};
                control.gpio_set(0, false).await;
                Timer::after_secs(1).await;
                control.gpio_set(0, true).await;
                Timer::after_secs(sample[0] as u64).await;
            }
            grant_r.release(frame_size);
        }
        else{
            if led_on{
                info!("SETTING LED OFF");
                control.gpio_set(0, false).await;
                led_on = false;
            }
        }
    }
}

