#![no_std]
#![no_main]

use core::fmt::Write;
use embassy_executor::Spawner;
use embassy_net::{
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    Config, Stack, StackResources,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, Sender},
};
use embassy_time::{with_timeout, Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::{ClockControl, Clocks},
    gpio::{AnyInput, Io, Level, Pull},
    peripherals::{Peripherals, RADIO_CLK, TIMG0, WIFI},
    prelude::*,
    reset::software_reset,
    rng::Rng,
    system::SystemControl,
    timer::{
        systimer::{SystemTimer, Target},
        timg::TimerGroup,
    },
};
use esp_wifi::{
    wifi::{
        new_with_mode, ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent,
        WifiStaDevice, WifiState,
    },
    EspWifiInitFor,
};
use heapless::String;
use reqwless::{client::HttpClient, request};

// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

// loaded from .env file by build.rs (alternatively, see dotenvy_macro::dotenv!() - example below)
// const SSID: &str = dotenv!("SSID");S
const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");
const BASE_URL: &str = env!("URL"); //example: `XXX.XXX.XX.XX:3000`
const DEBOUNCE_DELAY: Duration = Duration::from_millis(1);

#[main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    log::info!("URL={BASE_URL}");
    log::info!("SSID={SSID}");
    log::info!("PASSWORD={PASSWORD}");

    // initialize peripherals
    // TODO: when esp-hal 0.21 is released, simplify initialization and remove &clocks refs throughout
    let peripherals = Peripherals::take();
    let system = SystemControl::new(peripherals.SYSTEM);
    let clocks = ClockControl::max(system.clock_control).freeze();
    let rng = Rng::new(peripherals.RNG);

    // initialize embassy
    let systimer = SystemTimer::new(peripherals.SYSTIMER).split::<Target>();
    esp_hal_embassy::init(&clocks, systimer.alarm0);

    // initialize wifi
    let stack = init_for_wifi(
        peripherals.TIMG0,
        peripherals.RADIO_CLK,
        peripherals.WIFI,
        rng,
        &clocks,
        &spawner,
    )
    .await;

    // TODO: when embassy_net 0.5.0 is released: `tcp_client.set_timeout(Some(Duration::from_millis(10_000)));`
    // Once implemented, the match arms in notify_server() can be simplified

    // create http_client to manage HTTP requests
    let client_state = TcpClientState::<1, 1024, 1024>::new();
    let tcp_client = TcpClient::new(stack, &client_state);
    let dns_client = DnsSocket::new(stack);
    let mut http_client = HttpClient::new(&tcp_client, &dns_client);

    // initialize hall effect sensor & channel
    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let hall_sensor = AnyInput::new(io.pins.gpio10, Pull::Up);
    let mut hall_sensor_state = hall_sensor.get_level();
    static CHANNEL: Channel<CriticalSectionRawMutex, Level, 8> = Channel::new();
    let sender = CHANNEL.sender();
    spawner.spawn(sensor_watcher(hall_sensor, sender)).unwrap();

    // monitor hall effect sensor; notify server of changes
    loop {
        // TODO: replace dummy ID with UUID of MAC address from MCU
        let id = 1;
        if let Ok(level) = with_timeout(Duration::from_secs(5), CHANNEL.receive()).await {
            hall_sensor_state = level;
        }
        // sensor status: 0 -> open, 1 -> closed
        let status = !bool::from(hall_sensor_state);
        let url = build_url(BASE_URL, id, status).await;
        notify_server(&mut http_client, &url).await;
    }
}

async fn init_for_wifi(
    timer: TIMG0,
    radio: RADIO_CLK,
    wifi: WIFI,
    mut rng: Rng,
    clocks: &Clocks<'_>,
    spawner: &Spawner,
) -> &'static Stack<WifiDevice<'static, WifiStaDevice>> {
    // initialize hardware
    let timer = TimerGroup::new(timer, clocks).timer0;
    let init = esp_wifi::initialize(EspWifiInitFor::Wifi, timer, rng, radio, clocks).unwrap();
    let (wifi_interface, controller) = new_with_mode(&init, wifi, WifiStaDevice).unwrap();

    // initialize wifi stack
    let config = Config::dhcpv4(Default::default());
    let seed = rng.random().into();
    let stack = &*mk_static!(
        Stack<WifiDevice<'_, WifiStaDevice>>,
        Stack::new(
            wifi_interface,
            config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            seed
        )
    );

    // spawn background tasks to manage wifi connection and run network tasks
    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(stack)).ok();

    // wait for DHCP server to provide IP address
    log::info!("Waiting to get IP address...");
    while !stack.is_link_up() {
        Timer::after(Duration::from_millis(500)).await;
    }
    loop {
        if let Some(config) = stack.config_v4() {
            log::info!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    stack
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    log::info!("start connection task");
    log::info!("Device capabilities: {:?}", controller.get_capabilities());
    loop {
        if esp_wifi::wifi::get_wifi_state() == WifiState::StaConnected {
            // wait until we're no longer connected
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_millis(5000)).await
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.try_into().unwrap(),
                password: PASSWORD.try_into().unwrap(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            log::info!("Starting wifi");
            controller.start().await.unwrap();
            log::info!("Wifi started!");
        }
        log::info!("About to connect...");

        match controller.connect().await {
            Ok(_) => log::info!("Wifi connected!"),
            Err(e) => {
                log::info!("Failed to connect to wifi: {e:?}");
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    stack.run().await
}

#[embassy_executor::task]
async fn sensor_watcher(
    mut hall_sensor: AnyInput<'static>,
    sender: Sender<'static, CriticalSectionRawMutex, Level, 8>,
) {
    loop {
        hall_sensor.wait_for_any_edge().await;
        Timer::after(DEBOUNCE_DELAY).await;
        let state = hall_sensor.get_level();
        log::info!("Pin change detected. Level: {state:?}");
        sender.send(state).await;
    }
}

/// Constructs request URL to notify server of sensor status.
/// If specified base_url is invalid, will wait 30 seconds then reset.
async fn build_url(base_url: &str, id: i32, status: bool) -> String<128> {
    log::info!("Building URL");
    let mut url = String::new();
    match write!(&mut url, "http://{base_url}/api/{id}/{status}") {
        Ok(url) => url,
        Err(e) => {
            log::error!("Failed to build URL: {e:?}\nResetting after 30 seconds...");
            Timer::after(Duration::from_secs(30)).await;
            software_reset();
        }
    };
    url
}

/// Send current status of sensor to server.
async fn notify_server(
    http_client: &mut HttpClient<
        '_,
        TcpClient<'_, WifiDevice<'static, WifiStaDevice>, 1>,
        DnsSocket<'_, WifiDevice<'static, WifiStaDevice>>,
    >,
    url: &String<128>,
) {
    log::info!("Making request (url: {url})");
    let mut rx_buffer = [0; 4096];

    let timeout = with_timeout(Duration::from_secs(10), async {
        let mut request = match http_client.request(request::Method::POST, url).await {
            Ok(req) => req,
            Err(e) => {
                log::error!("Failed to make HTTP request: {:?}", e);
                return;
            }
        };
        match request.send(&mut rx_buffer).await {
            Ok(resp) => log::info!("Response status: {:?}", resp.status),
            Err(e) => {
                log::error!("Failed to send HTTP request: {:?}", e);
            }
        };
    })
    .await;

    if timeout.is_err() {
        log::error!("Request failed: Timeout")
    };
}
