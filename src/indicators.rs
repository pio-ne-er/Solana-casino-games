// Technical indicators: RSI, MACD, Momentum

use std::collections::VecDeque;

/// Rolling RSI calculator using VecDeque for efficient updates
pub struct RollingRSI {
    period: usize,
    prices: VecDeque<f64>,
    gains: VecDeque<f64>,
    losses: VecDeque<f64>,
    avg_gain: f64,
    avg_loss: f64,
    initialized: bool,
}

impl RollingRSI {
    pub fn new(period: usize) -> Self {
        Self {
            period,
            prices: VecDeque::with_capacity(period + 1),
            gains: VecDeque::with_capacity(period),
            losses: VecDeque::with_capacity(period),
            avg_gain: 0.0,
            avg_loss: 0.0,
            initialized: false,
        }
    }

    /// Add a new price and update RSI calculation
    pub fn add_price(&mut self, price: f64) {
        self.prices.push_back(price);
        
        if self.prices.len() < 2 {
            return;
        }

        // Calculate price change
        let prev_price = self.prices[self.prices.len() - 2];
        let change = price - prev_price;
        
        let gain = if change > 0.0 { change } else { 0.0 };
        let loss = if change < 0.0 { -change } else { 0.0 };

        self.gains.push_back(gain);
        self.losses.push_back(loss);

        if !self.initialized {
            // Initialize with first period values
            if self.gains.len() == self.period {
                self.avg_gain = self.gains.iter().sum::<f64>() / self.period as f64;
                self.avg_loss = self.losses.iter().sum::<f64>() / self.period as f64;
                self.initialized = true;
            }
        } else {
            // Use Wilder's smoothing method for rolling updates
            self.avg_gain = (self.avg_gain * (self.period as f64 - 1.0) + gain) / self.period as f64;
            self.avg_loss = (self.avg_loss * (self.period as f64 - 1.0) + loss) / self.period as f64;

            // Remove oldest values if deque is full
            if self.gains.len() > self.period {
                self.gains.pop_front();
            }
            if self.losses.len() > self.period {
                self.losses.pop_front();
            }
        }

        // Keep prices deque size manageable
        if self.prices.len() > self.period + 1 {
            self.prices.pop_front();
        }
    }

    /// Get current RSI value
    pub fn get_rsi(&self) -> Option<f64> {
        if !self.initialized {
            return None;
        }
        // Standard handling:
        // - If both avg_gain and avg_loss are ~0 (completely flat), treat as neutral RSI ~ 50
        // - If only avg_loss is 0, RSI = 100 (only gains)
        // - If only avg_gain is 0, RSI = 0 (only losses)
        if self.avg_loss == 0.0 && self.avg_gain == 0.0 {
            return Some(50.0);
        } else if self.avg_loss == 0.0 {
            return Some(100.0);
        } else if self.avg_gain == 0.0 {
            return Some(0.0);
        }

        let rs = self.avg_gain / self.avg_loss;
        let rsi = 100.0 - (100.0 / (1.0 + rs));

        Some(rsi)
    }
    
    /// Debug: Get recent prices (for debugging)
    #[allow(dead_code)]
    pub fn get_recent_prices(&self) -> Vec<f64> {
        self.prices.iter().copied().collect()
    }
    
    /// Debug: Get avg_gain and avg_loss (for debugging)
    #[allow(dead_code)]
    pub fn get_stats(&self) -> (f64, f64) {
        (self.avg_gain, self.avg_loss)
    }

    /// Check if we have enough data
    pub fn is_ready(&self) -> bool {
        self.initialized
    }
}

/// Rolling MACD calculator using VecDeque for efficient updates
pub struct RollingMACD {
    fast_period: usize,  // EMA12
    slow_period: usize,  // EMA26
    signal_period: Option<usize>,  // Signal line period (None for regular MACD, Some(9) for signal line)
    prices: VecDeque<f64>,
    ema_fast: f64,
    ema_slow: f64,
    macd_history: VecDeque<f64>,  // Track MACD values for signal line calculation
    signal_line: f64,  // EMA of MACD Line
    signal_initialized: bool,
    initialized: bool,
}

impl RollingMACD {
    pub fn new(fast_period: usize, slow_period: usize) -> Self {
        Self {
            fast_period,
            slow_period,
            signal_period: None,  // Regular MACD mode
            prices: VecDeque::with_capacity(slow_period + 1),
            ema_fast: 0.0,
            ema_slow: 0.0,
            macd_history: VecDeque::new(),
            signal_line: 0.0,
            signal_initialized: false,
            initialized: false,
        }
    }
    
    pub fn new_with_signal(fast_period: usize, slow_period: usize, signal_period: usize) -> Self {
        Self {
            fast_period,
            slow_period,
            signal_period: Some(signal_period),
            prices: VecDeque::with_capacity(slow_period + 1),
            ema_fast: 0.0,
            ema_slow: 0.0,
            macd_history: VecDeque::with_capacity(signal_period + 1),
            signal_line: 0.0,
            signal_initialized: false,
            initialized: false,
        }
    }

    /// Add a new price and update MACD calculation
    pub fn add_price(&mut self, price: f64) {
        self.prices.push_back(price);

        if !self.initialized {
            // Initialize EMAs with SMA when we have enough data
            if self.prices.len() >= self.slow_period {
                let sum: f64 = self.prices.iter().sum();
                let count = self.prices.len() as f64;
                let sma = sum / count;
                self.ema_fast = sma;
                self.ema_slow = sma;
                self.initialized = true;
            }
        } else {
            // Update EMAs using exponential smoothing
            let fast_alpha = 2.0 / (self.fast_period as f64 + 1.0);
            let slow_alpha = 2.0 / (self.slow_period as f64 + 1.0);
            
            self.ema_fast = (price * fast_alpha) + (self.ema_fast * (1.0 - fast_alpha));
            self.ema_slow = (price * slow_alpha) + (self.ema_slow * (1.0 - slow_alpha));
        }

        // Calculate MACD value
        if self.initialized {
            let macd_value = self.ema_fast - self.ema_slow;
            
            // If signal line mode, track MACD history and calculate signal line
            if let Some(signal_period) = self.signal_period {
                self.macd_history.push_back(macd_value);
                
                // Keep history size manageable
                if self.macd_history.len() > signal_period + 1 {
                    self.macd_history.pop_front();
                }
                
                // Calculate signal line (EMA of MACD)
                if !self.signal_initialized {
                    if self.macd_history.len() >= signal_period {
                        // Initialize signal line with SMA of MACD values
                        let sum: f64 = self.macd_history.iter().sum();
                        let count = self.macd_history.len() as f64;
                        self.signal_line = sum / count;
                        self.signal_initialized = true;
                    }
                } else {
                    // Update signal line using EMA
                    let signal_alpha = 2.0 / (signal_period as f64 + 1.0);
                    self.signal_line = (macd_value * signal_alpha) + (self.signal_line * (1.0 - signal_alpha));
                }
            }
        }

        // Keep prices deque size manageable
        if self.prices.len() > self.slow_period + 1 {
            self.prices.pop_front();
        }
    }

    /// Get current MACD value (EMA12 - EMA26)
    pub fn get_macd(&self) -> Option<f64> {
        if !self.initialized {
            return None;
        }
        Some(self.ema_fast - self.ema_slow)
    }
    
    /// Get signal line value (only available if signal_period is set)
    pub fn get_signal_line(&self) -> Option<f64> {
        if self.signal_period.is_some() && self.signal_initialized {
            Some(self.signal_line)
        } else {
            None
        }
    }
    
    /// Get histogram (MACD - Signal Line)
    pub fn get_histogram(&self) -> Option<f64> {
        match (self.get_macd(), self.get_signal_line()) {
            (Some(macd), Some(signal)) => Some(macd - signal),
            _ => None,
        }
    }

    /// Check if we have enough data
    pub fn is_ready(&self) -> bool {
        self.initialized
    }
    
    /// Check if signal line is ready (for MACDSignal mode)
    pub fn is_signal_ready(&self) -> bool {
        self.signal_initialized
    }
}

/// Rolling Momentum calculator using VecDeque for efficient updates
pub struct RollingMomentum {
    period: usize,
    prices: VecDeque<f64>,
}

impl RollingMomentum {
    pub fn new(period: usize) -> Self {
        Self {
            period,
            prices: VecDeque::with_capacity(period + 1),
        }
    }

    /// Add a new price
    pub fn add_price(&mut self, price: f64) {
        self.prices.push_back(price);
        if self.prices.len() > self.period + 1 {
            self.prices.pop_front();
        }
    }

    /// Get current momentum (percentage change over period)
    pub fn get_momentum(&self) -> Option<f64> {
        if self.prices.len() < self.period + 1 {
            return None;
        }
        let current = *self.prices.back().unwrap();
        let past = *self.prices.front().unwrap();
        if past == 0.0 {
            return None;
        }
        Some(((current - past) / past) * 100.0) // Return as percentage
    }

    /// Check if we have enough data
    pub fn is_ready(&self) -> bool {
        self.prices.len() >= self.period + 1
    }
}

/// Calculate RSI (Relative Strength Index) for a given period (legacy function for compatibility)
/// Returns None if there's insufficient data
pub fn calculate_rsi(prices: &[f64], period: usize) -> Option<f64> {
    if prices.len() < period + 1 {
        return None;
    }

    let mut gains = Vec::new();
    let mut losses = Vec::new();

    // Calculate price changes
    for i in 1..prices.len() {
        let change = prices[i] - prices[i - 1];
        if change > 0.0 {
            gains.push(change);
            losses.push(0.0);
        } else {
            gains.push(0.0);
            losses.push(-change);
        }
    }

    // Calculate initial average gain and loss
    let mut avg_gain = gains[..period].iter().sum::<f64>() / period as f64;
    let mut avg_loss = losses[..period].iter().sum::<f64>() / period as f64;

    // Use Wilder's smoothing method for subsequent values
    for i in period..gains.len() {
        avg_gain = (avg_gain * (period as f64 - 1.0) + gains[i]) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + losses[i]) / period as f64;
    }

    // Mirror the same edge-case handling as RollingRSI::get_rsi
    if avg_loss == 0.0 && avg_gain == 0.0 {
        return Some(50.0);
    } else if avg_loss == 0.0 {
        return Some(100.0);
    } else if avg_gain == 0.0 {
        return Some(0.0);
    }

    let rs = avg_gain / avg_loss;
    let rsi = 100.0 - (100.0 / (1.0 + rs));

    Some(rsi)
}
