#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(stable_features, unknown_lints, async_fn_in_trait)]
#![feature(alloc)]


extern crate alloc;
use alloc::vec::Vec;
use bbqueue::BBBuffer;
use cortex_m::Peripherals;
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
use embedded_alloc::Heap;
use static_cell::make_static;
use {defmt_rtt as _, panic_probe as _};
use bbqueue::Error as QError;
use embassy_rp::multicore::spawn_core1;
use embassy_rp::multicore::Stack as MStack;
use static_cell::StaticCell;
use embassy_executor::Executor;
mod queue;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use queue::{decode_task,enqueue_bytes};
use bbqueue::{Producer,Consumer};
use cortex_m::Peripherals as CortexPeripherals;
use queue::dequeue_frame_size;
use cyw43::Control;
use core::mem::transmute;
use rmp3::Sample;
enum LedState {
    On,
    Off,
}

#[global_allocator]
static HEAP: Heap = Heap::empty();

static mut CORE1_STACK: MStack<4096> = MStack::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();
static CHANNEL: Channel<CriticalSectionRawMutex, LedState, 1> = Channel::new();


const SAMPLE_RATE: usize = 8000; // 8-24KHz
const BIT_RATE: usize = 64; // 64 Bit/s
const BUFFER_SIZE: usize = 10240;
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

static DECODE_QUEUE: BBBuffer<BUFFER_SIZE> = BBBuffer::new();
static PLAY_QUEUE: BBBuffer<BUFFER_SIZE> = BBBuffer::new();

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
pub async fn play_task(mut control: Control<'static>, mut consumer: Consumer<'static, BUFFER_SIZE>)->! {
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

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

#[cortex_m_rt::entry]
fn main() -> ! {
    info!("PROGRAM START");
    let p = embassy_rp::init(Default::default());
    let _music = include_bytes!("../../../Mr_Blue_Sky-Electric_Light_Orchestra-trimmed.mp3");

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| unwrap!(spawner.spawn(core0_task(spawner,p,/*  play_producer, decode_consumer, decode_producer */))));
}

#[embassy_executor::task]
async fn core0_task(
    spawner: Spawner,
    p: embassy_rp::Peripherals, 
    // play_producer: Producer<'static, BUFFER_SIZE>,
    // decode_consumer: Consumer<'static, BUFFER_SIZE>, 
    // mut decode_producer: Producer<'static,BUFFER_SIZE>
) {
    info!("Hello from core 0");
    let fw = include_bytes!("../../embassy/cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../../embassy/cyw43-firmware/43439A0_clm.bin");
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
    let (mut decode_producer, decode_consumer) = DECODE_QUEUE.try_split().unwrap();
    let (play_producer, play_consumer) = PLAY_QUEUE.try_split().unwrap();

    let state = make_static!(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(wifi_task(runner)));
    info!("STARTING WIFI_TASK");
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = Config::dhcpv4(Default::default());
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
    spawn_core1(p.CORE1, unsafe { &mut CORE1_STACK }, move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| unwrap!(spawner.spawn(core1_task(control,play_consumer))));
    });

    unwrap!(spawner.spawn(decode_task(decode_consumer, play_producer)));
    loop {
        let mut rx_buffer = [0; 4096];
        let mut tx_buffer = [0; 4096];
        let mut buf = [0; 4096];



        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(1000)));

        // control.gpio_set(0, false).await;
        info!("Listening on TCP:1234...");
        // On error warn and continue
        if let Err(e) = socket.accept(1234).await {
            warn!("accept error: {:?}", e);
            continue;
        }
        info!("Received connection from {:?}", socket.remote_endpoint());
        // control.gpio_set(0, true).await;
        loop {
            match socket.read(&mut buf).await {
                Ok(0) => {
                    info!("Read EOF!");
                    break;
                }
                Ok(d) => {
                    enqueue_bytes(&mut decode_producer, &buf, d); 
                    info!("Received {:?} bytes", d);
                    d
                }
                Err(e) => {
                    info!("error! {:?}", e);
                    0
                }   
            };
        }
    }
}

#[embassy_executor::task]
async fn core1_task(control: Control<'static>,consumer: Consumer<'static,BUFFER_SIZE>) {
    play_task(control,consumer);
    
    // let led = LedState::new();
        info!("Hello from core 1");
    // loop {
    //     match CHANNEL.receive().await {
    //         // LedState::On => led.set_high(),
    //         // LedState::Off => led.set_low(),
    //     }
    // }
}
