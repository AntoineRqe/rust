/// Pre-parsed FIX ExecutionReport data extracted by the execution-report engine.
/// Avoids redundant FIX parsing in the fix_session layer.
#[derive(Clone, Debug)]
pub struct ExecReportData {
    pub order_id: u64,     // Stable hash of effective_cl_ord_id
    pub cl_ord_id: String, // Effective ClOrdId of the target order
    pub symbol: String,    // FIX field 55
    pub side: u8,          // FIX field 54 (1=Buy, 2=Sell)
    pub ord_status: u8,    // FIX field 39 (0=New, 1=PartialFill, 2=Fill, 3=DoneForDay, 4=Canceled)
    pub price: f64,        // FIX field 44
    pub qty: f64,          // FIX field 38
    pub leaves_qty: f64,   // FIX field 151
}

/// Bundles a FIX execution report message with its pre-parsed ExecutionReport data
/// to avoid redundant parsing in the fix_session layer.
/// Stores FIX message as len + bytes to avoid circular dependency on fix crate.
#[derive(Clone)]
pub struct ExecutionReportMessage<const N: usize> {
    /// The raw FIX message length
    pub fix_len: u16,
    /// The raw FIX message bytes (for transmission to clients)
    pub fix_data: [u8; N],
    /// The pre-parsed execution report data (for order book updates)
    pub exec_report_data: ExecReportData,
}

impl<const N: usize> std::fmt::Debug for ExecutionReportMessage<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionReportMessage")
            .field("fix_len", &self.fix_len)
            .field("fix_data_len", &N)
            .field("exec_report_data", &self.exec_report_data)
            .finish()
    }
}

impl<const N: usize> ExecutionReportMessage<N> {
    pub fn new(
        fix_len: u16,
        fix_data: [u8; N],
        exec_report_data: ExecReportData,
    ) -> Self {
        Self {
            fix_len,
            fix_data,
            exec_report_data,
        }
    }
}

