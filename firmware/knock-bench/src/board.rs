//! Board support for the B-L475E-IOT01A Discovery kit (STM32L475VG).
//!
//! [`Board::init`] configures the clocks and hands out every peripheral the
//! bench tests use. Call it exactly once, first thing in `main`.
//!
//! # Pin map
//!
//! | Function | Pins | Board net |
//! |---|---|---|
//! | I2C1 (BMI160s + TCA9548A) | SCL = PB8, SDA = PB9 | Arduino D15 / D14 |
//! | I2C2 (onboard LSM6DSL, 0x6A) | SCL = PB10, SDA = PB11 | internal sensor bus |
//! | USART1 (ST-LINK virtual COM port) | TX = PB6, RX = PB7 | ST-LINK |
//! | LED2 (green) | PB14, active high | LD2 |
//! | User button (blue) | PC13, active low | B2 |
//! | Piezo ADC inputs A0..A3 | PC5, PC4, PC3, PC2 | Arduino A0..A3 |
//! | Spare pin (logic-analyzer probe of `t04`) | PD14 | Arduino D2 |
//!
//! Every alternate function and DMA request used here exists in the
//! `stm32-metapac` tables for the STM32L475VG (otherwise this file would not
//! compile). The *board-level* nets (which header pin goes to which MCU pin)
//! were cross-checked against the Zephyr board files for `disco_l475_iot1`
//! (`disco_l475_iot1.dts`, `arduino_r3_connector.dtsi`), not against ST's user
//! manual UM2153 itself, which is not in `references/`.
//!
//! ADC channels for the piezo inputs: PC5 = ADC1_IN14, PC4 = ADC1_IN13,
//! PC3 = ADC1_IN4, PC2 = ADC1_IN3.
//!
//! # DMA channels and interrupts
//!
//! | Peripheral | TX | RX |
//! |---|---|---|
//! | I2C1 | DMA1_CH6 | DMA1_CH7 |
//! | I2C2 | DMA1_CH4 | DMA1_CH5 |
//! | USART1 | DMA2_CH6 | DMA2_CH7 |
//!
//! ADC1 can use DMA1_CH1 (handed out in [`PiezoResources`]) or DMA2_CH3.
//!
//! [`Irqs`] binds the interrupt vectors for all of the above plus EXTI15_10
//! (the button). A module that needs further interrupts (for example
//! `ADC1_2` or `DMA1_CHANNEL1`) declares its own `bind_interrupts!` struct;
//! that is fine as long as no vector is bound twice.
//!
//! # Clocks
//!
//! SYSCLK = 80 MHz from HSI16 through the main PLL (16 MHz / 1 × 10 / 2), with
//! AHB, APB1 and APB2 undivided. HSI16 is used rather than MSI because its
//! factory trim (±1 %) is good enough for 921600 baud without depending on the
//! LSE crystal. The ADC kernel clock is set to SYSCLK so ADC1 is usable.
//!
//! The embassy time driver (`time-driver-any`) claims TIM15 on this chip
//! (embassy-stm32 0.6.0 prefers the 2-channel timers), so TIM15 is not
//! available for ADC triggering or PWM.

use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::{Level, Output, Pull, Speed};
use embassy_stm32::i2c::{self, I2c};
use embassy_stm32::mode::Async;
use embassy_stm32::time::Hertz;
use embassy_stm32::usart::{self, Uart};
use embassy_stm32::{Peri, bind_interrupts, dma, exti, interrupt, peripherals, rcc};
use embassy_time::{Duration, Timer};

use crate::recovery::{self, BusState};

bind_interrupts!(
    /// Interrupt bindings for every peripheral [`Board::init`] hands out.
    pub struct Irqs {
        I2C1_EV => i2c::EventInterruptHandler<peripherals::I2C1>;
        I2C1_ER => i2c::ErrorInterruptHandler<peripherals::I2C1>;
        I2C2_EV => i2c::EventInterruptHandler<peripherals::I2C2>;
        I2C2_ER => i2c::ErrorInterruptHandler<peripherals::I2C2>;
        USART1 => usart::InterruptHandler<peripherals::USART1>;
        DMA1_CHANNEL4 => dma::InterruptHandler<peripherals::DMA1_CH4>;
        DMA1_CHANNEL5 => dma::InterruptHandler<peripherals::DMA1_CH5>;
        DMA1_CHANNEL6 => dma::InterruptHandler<peripherals::DMA1_CH6>;
        DMA1_CHANNEL7 => dma::InterruptHandler<peripherals::DMA1_CH7>;
        DMA2_CHANNEL6 => dma::InterruptHandler<peripherals::DMA2_CH6>;
        DMA2_CHANNEL7 => dma::InterruptHandler<peripherals::DMA2_CH7>;
        EXTI15_10 => exti::InterruptHandler<interrupt::typelevel::EXTI15_10>;
    }
);

/// System clock frequency set by [`Board::init`].
pub const SYSCLK_HZ: u32 = 80_000_000;

/// I2C1 bus speed: 400 kHz, or 100 kHz with the `i2c-100k` cargo feature.
#[cfg(not(feature = "i2c-100k"))]
pub const I2C1_HZ: u32 = 400_000;
/// I2C1 bus speed: 400 kHz, or 100 kHz with the `i2c-100k` cargo feature.
#[cfg(feature = "i2c-100k")]
pub const I2C1_HZ: u32 = 100_000;

/// I2C2 (onboard sensors) bus speed.
pub const I2C2_HZ: u32 = 400_000;

/// Baud rate of the ST-LINK virtual COM port (8N1).
pub const VCP_BAUD: u32 = 921_600;

/// Timeout of the I2C driver's own transfer guard (see [`crate::bus`]).
pub const I2C_HAL_TIMEOUT: Duration = Duration::from_millis(250);

/// An async I2C master as handed out by this module.
pub type I2cBus<'d> = I2c<'d, Async, i2c::Master>;

/// The ST-LINK virtual COM port.
pub type Vcp = Uart<'static, Async>;

/// Everything the bench tests use, ready configured.
pub struct Board {
    /// I2C1 on Arduino D15/D14: the BMI160 modules and the TCA9548A.
    pub i2c1: I2c1Port,
    /// I2C2: the onboard sensor bus (LSM6DSL at 0x6A, among others).
    pub i2c2: I2cBus<'static>,
    /// USART1 at [`VCP_BAUD`]: the ST-LINK virtual COM port.
    pub vcp: Vcp,
    /// LED2 (green), active high, initially off.
    pub led2: Output<'static>,
    /// The blue user button, active low. See [`wait_for_press`].
    pub button: ExtiInput<'static, Async>,
    /// ADC1 and the four piezo input pins, unconfigured.
    pub piezo: PiezoResources,
    /// The spare pin, Arduino D2 = PD14, unconfigured (reset state). Turn it
    /// into the logic-analyzer probe output with [`crate::imu::probe_pin`].
    // UNVERIFIED: "Arduino D2 = PD14" is from the Zephyr board files for
    // disco_l475_iot1 (arduino_r3_connector.dtsi) as remembered, not from
    // UM2153, which is not in `references/`.
    pub probe: Peri<'static, peripherals::PD14>,
    /// The independent watchdog, not started. Start it with
    /// [`crate::imu::start_watchdog`]; it cannot be stopped again.
    pub iwdg: Peri<'static, peripherals::IWDG>,
}

/// ADC1 and the piezo input pins, left unconfigured for the piezo tests.
///
/// The pins are in their reset state (analog mode). The ADC kernel clock is
/// already routed (SYSCLK, 80 MHz).
pub struct PiezoResources {
    /// The ADC used for the piezo channels.
    pub adc: Peri<'static, peripherals::ADC1>,
    /// A DMA channel that can serve ADC1 (request 0). Its interrupt
    /// (`DMA1_CHANNEL1`) is not bound by [`Irqs`].
    pub adc_dma: Peri<'static, peripherals::DMA1_CH1>,
    /// Arduino A0 = PC5 = ADC1_IN14.
    pub a0: Peri<'static, peripherals::PC5>,
    /// Arduino A1 = PC4 = ADC1_IN13.
    pub a1: Peri<'static, peripherals::PC4>,
    /// Arduino A2 = PC3 = ADC1_IN4.
    pub a2: Peri<'static, peripherals::PC3>,
    /// Arduino A3 = PC2 = ADC1_IN3.
    pub a3: Peri<'static, peripherals::PC2>,
}

/// Owner of the I2C1 peripheral and its pins.
///
/// The port lends its resources to a bus ([`I2c1Port::bus`]) or to the
/// recovery routine ([`I2c1Port::recover`]), never both at once. See
/// [`crate::recovery`] for the reasoning and a usage example.
pub struct I2c1Port {
    peri: Peri<'static, peripherals::I2C1>,
    scl: Peri<'static, peripherals::PB8>,
    sda: Peri<'static, peripherals::PB9>,
    tx_dma: Peri<'static, peripherals::DMA1_CH6>,
    rx_dma: Peri<'static, peripherals::DMA1_CH7>,
}

impl I2c1Port {
    /// Build the I2C1 master at [`I2C1_HZ`].
    ///
    /// The peripheral is reset and initialised from scratch on every call, and
    /// disabled again when the returned bus is dropped.
    ///
    /// The MCU's internal pull-ups (about 40 kΩ) are enabled on SCL and SDA.
    /// They are far too weak to matter next to the modules' own pull-ups, but
    /// they keep an *empty* bus idle-high, so a scan with nothing connected
    /// gets clean NACKs instead of a stuck-bus timeout.
    pub fn bus(&mut self) -> I2cBus<'_> {
        let mut config = i2c::Config::default();
        config.frequency = Hertz(I2C1_HZ);
        config.scl_pullup = true;
        config.sda_pullup = true;
        config.timeout = I2C_HAL_TIMEOUT;
        I2c::new(
            self.peri.reborrow(),
            self.scl.reborrow(),
            self.sda.reborrow(),
            self.tx_dma.reborrow(),
            self.rx_dma.reborrow(),
            Irqs,
            config,
        )
    }

    /// Run I2C bus recovery (9 SCL pulses + STOP) on PB8/PB9.
    ///
    /// Takes about 2.3 ms. Create a new bus with [`I2c1Port::bus`] afterwards.
    pub async fn recover(&mut self) -> BusState {
        recovery::recover_pins(self.scl.reborrow(), self.sda.reborrow()).await
    }
}

impl Board {
    /// Configure clocks and peripherals. Call once at the start of `main`.
    ///
    /// # Panics
    ///
    /// Panics if called twice (the peripherals can only be taken once).
    pub fn init() -> Self {
        let mut config = embassy_stm32::Config::default();
        config.rcc.hsi = true;
        config.rcc.pll = Some(rcc::Pll {
            source: rcc::PllSource::HSI,
            prediv: rcc::PllPreDiv::DIV1,
            mul: rcc::PllMul::MUL10,
            divp: None,
            divq: None,
            divr: Some(rcc::PllRDiv::DIV2), // 16 MHz × 10 / 2 = 80 MHz
        });
        config.rcc.sys = rcc::Sysclk::PLL1_R;
        config.rcc.mux.adcsel = rcc::mux::Adcsel::SYS;
        let p = embassy_stm32::init(config);

        let i2c1 = I2c1Port {
            peri: p.I2C1,
            scl: p.PB8,
            sda: p.PB9,
            tx_dma: p.DMA1_CH6,
            rx_dma: p.DMA1_CH7,
        };

        // The onboard sensor bus has its own pull-ups on the board.
        let mut i2c2_config = i2c::Config::default();
        i2c2_config.frequency = Hertz(I2C2_HZ);
        i2c2_config.timeout = I2C_HAL_TIMEOUT;
        let i2c2 = I2c::new(
            p.I2C2,
            p.PB10,
            p.PB11,
            p.DMA1_CH4,
            p.DMA1_CH5,
            Irqs,
            i2c2_config,
        );

        let mut vcp_config = usart::Config::default();
        vcp_config.baudrate = VCP_BAUD;
        let vcp = match Uart::new(
            p.USART1, p.PB7, p.PB6, p.DMA2_CH6, p.DMA2_CH7, Irqs, vcp_config,
        ) {
            Ok(uart) => uart,
            // Only reachable if VCP_BAUD cannot be derived from the 80 MHz clock.
            Err(e) => defmt::panic!("USART1 config rejected: {}", e),
        };

        let led2 = Output::new(p.PB14, Level::Low, Speed::Low);
        let button = ExtiInput::new(p.PC13, p.EXTI13, Pull::Up, Irqs);

        Board {
            i2c1,
            i2c2,
            vcp,
            led2,
            button,
            piezo: PiezoResources {
                adc: p.ADC1,
                adc_dma: p.DMA1_CH1,
                a0: p.PC5,
                a1: p.PC4,
                a2: p.PC3,
                a3: p.PC2,
            },
            probe: p.PD14,
            iwdg: p.IWDG,
        }
    }
}

/// Debounce time used by [`wait_for_press`].
pub const BUTTON_DEBOUNCE: Duration = Duration::from_millis(30);

/// Wait for a debounced press of the (active-low) user button.
///
/// Returns once the button has been released and then held down for
/// [`BUTTON_DEBOUNCE`]; a button that is already down when this is called must
/// be released first, so one long press is not counted twice.
pub async fn wait_for_press(button: &mut ExtiInput<'static, Async>) {
    loop {
        button.wait_for_high().await;
        Timer::after(BUTTON_DEBOUNCE).await;
        if button.is_low() {
            continue;
        }
        button.wait_for_low().await;
        Timer::after(BUTTON_DEBOUNCE).await;
        if button.is_low() {
            return;
        }
    }
}
