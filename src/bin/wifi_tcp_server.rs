#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(stable_features, unknown_lints, async_fn_in_trait)]
#![feature(alloc)]

extern crate alloc;
use alloc::vec::Vec;
use core::mem::transmute;
use cyw43_pio::PioSpi;
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, Stack, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_25, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use static_cell::make_static;
use {defmt_rtt as _, panic_probe as _};
use rmp3::{RawDecoder,Sample,MAX_SAMPLES_PER_FRAME};
use bbqueue::BBBuffer;
use bbqueue::{Producer,Consumer,GrantR,GrantW};


const SAMPLE_RATE: usize = 8000; // 8-24KHz
const BIT_RATE: usize = 64; // 64 Bit/s
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

static DECODE_QUEUE: BBBuffer<102400> = BBBuffer::new();
static PLAY_QUEUE: BBBuffer<102400> = BBBuffer::new();

const WIFI_NETWORK: &str = "TZ Guest";
const WIFI_PASSWORD: &str = "ilovetea";

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<
        'static,
        Output<'static, PIN_23>,
        PioSpi<'static, PIN_25, PIO0, 0, DMA_CH0>,
    >,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

// Decoder test should compare local decoding of included bytes with local deocding of bytestream
// received by TCP Socket
//
// Additional test should transmit decoded data back to client for comparison to data decoded by an
// alternative decoder library.
//
// test one should only test usage of rmp3 on the chip!!
fn decode_queue(
    play_producer: &mut Producer<'static, 102400>,
    src_buf: &[u8],
) {
    let mut dest = [Sample::default(); MAX_SAMPLES_PER_FRAME];
    let mut decoder = RawDecoder::new();
    match decoder.next(src_buf, &mut dest) {
        Some((_frame, bytes_decoded)) => {
            let dest: [u8; MAX_SAMPLES_PER_FRAME * 2] = unsafe { transmute(dest) };
            info!("successful byte decoding!");
            
            let mut header_grant = play_producer.grant_exact(1).unwrap();
            let header: [u8; 4] = unsafe { transmute([bytes_decoded]) };
            header_grant.buf().copy_from_slice(&header);
            header_grant.commit(4);

            let mut frame_grant = play_producer.grant_exact(bytes_decoded).unwrap();
            frame_grant.buf().copy_from_slice(&dest[..bytes_decoded]);
            frame_grant.commit(bytes_decoded);
        }
        None => {
            info!("error: decoder does not work...");
        }
    }
}
#[embassy_executor::task]
async fn decode_task(mut decode_consumer: Consumer<'static, 102400>, mut play_producer: Producer<'static, 102400>){
    loop{
        let read_buf = decode_consumer.read().unwrap();
        decode_queue(&mut play_producer, &read_buf); 
    }
}
#[embassy_executor::task]
async fn play_task(mut play_consumer: Consumer<'static, 102400>){
   loop{
        // let play_buf = read_from_queue(play_consumer);


    }
}
// fn read_f.copy_from_slice(&read_buf[..MAX_SAMPLES_PER_FRAME*2]);
//     read_buf.release(MAX_SAMPLES_PER_FRAME*2);
//     let play_buf: [Sample; MAX_SAMPLES_PER_FRAME] = unsafe {transmute(play_buf)};
//     play_buf

// }
fn read_header(grant_r: &mut GrantR<'static,102400>)->usize{
    let mut header = [0u8;4];
    header.copy_from_slice(&grant_r[..3]);
    let header: [usize;1] = unsafe{transmute(header)};
    header[0]

}
fn play(mut play_consumer: Consumer<'static, 102400>)->[Sample;MAX_SAMPLES_PER_FRAMEax]{
    let read_buf = play_consumer.read().unwrap();
    let mut header = [0u8;4];
    header.copy_from_slice(&read_buf[..3]);
    let header: [usize;1] = unsafe{transmute(header)};
    let mut play_buf = [0u8; MAX_SAMPLES_PER_FRAME * 2];
    play_buf.copy_from_slice(&read_buf[..MAX_SAMPLES_PER_FRAME*2]);
    read_buf.release(MAX_SAMPLES_PER_FRAME*2);
    let play_buf: [Sample; MAX_SAMPLES_PER_FRAME] = unsafe {transmute(play_buf)};
    play_buf
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let music = include_bytes!("../../../Mr_Blue_Sky-Electric_Light_Orchestra-trimmed.mp3");

    let fw = include_bytes!("../../embassy/cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../../embassy/cyw43-firmware/43439A0_clm.bin");


    // test_decoder::<'a>(&mut decoder, music);
    // To make flashing faster for development, you may want to flash the firmwares independently
    // at hardcoded addresses, instead of baking them into the program with `include_bytes!`:
    //     probe-rs download 43439A0.bin --format bin --chip RP2040 --base-address 0x10100000
    //     probe-rs download 43439A0_clm.bin --format bin --chip RP2040 --base-address 0x10140000
    // let fw = unsafe { core::slice::from_raw_parts(0x10100000 as *const u8, 230321) };
    // let clm = unsafe { core::slice::from_raw_parts(0x10140000 as *const u8, 4752) };

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    let state = make_static!(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(wifi_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = Config::dhcpv4(Default::default());
    //let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
    //    address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 69, 2), 24),
    //    dns_servers: Vec::new(),
    //    gateway: Some(Ipv4Address::new(192, 168, 69, 1)),
    //});

    // Generate random seed
    let seed = 0x0123_4567_89ab_cdef; // chosen by fair dice roll. guarenteed to be random.
    let _vec: Vec<i32> = Vec::new();

    // Init network stack
    let stack = &*make_static!(Stack::new(
        net_device,
        config,
        make_static!(StackResources::<2>::new()),
        seed
    ));

    unwrap!(spawner.spawn(net_task(stack)));

    loop {
        // control.join_open(WIFI_NETWORK).await;
        match control.join_wpa2(WIFI_NETWORK, WIFI_PASSWORD).await {
            Ok(_) => break,
            Err(err) => {
                info!("join failed with status={}", err.status);
            }
        }
    }

    // Wait for DHCP, not necessary when using static IP
    info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");

    // And now we can use it!

    loop {
        let mut rx_buffer = [0; 4096];
        let mut tx_buffer = [0; 4096];
        let mut buf = [0; 4096];

        let (mut decode_prod, decode_cons) = DECODE_QUEUE.try_split().unwrap();
        let (mut play_prod, play_cons) = PLAY_QUEUE.try_split().unwrap();

        unwrap!(spawner.spawn(decode_task(decode_cons, play_prod)));

        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        control.gpio_set(0, false).await;
        info!("Listening on TCP:1234...");
        // On error warn and continue
        if let Err(e) = socket.accept(1234).await {
            warn!("accept error: {:?}", e);
            continue;
        }
        info!("Received connection from {:?}", socket.remote_endpoint());
        control.gpio_set(0, true).await;

        loop {
            let read = match socket.read(&mut buf).await {
                Ok(0) => {
                    info!("Read EOF!");
                    break;
                },
                Ok(d) => {
                    let mut grant = decode_prod.grant_exact(d).unwrap();
                    grant.buf().copy_from_slice(&buf[..d]);
                    grant.commit(d); 
                    info!("Received {:?} bytes", d);
                    d
                },
                Err(e) => {
                    info!("error! {:?}", e);
                    0
                },
            };
        }
    }
}
