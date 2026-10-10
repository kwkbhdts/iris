#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId {
    pub hwnd: usize,
    pub pid: u32,
    pub tid: u32,
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub id: usize,
    pub at: u32,
    pub foreground: Result<WindowId, u32>,
    pub focus: Result<WindowId, u32>,
}

// 配送不能と生の応答を区別し、不明をオフと決めつけない。
#[derive(Clone, Copy)]
pub enum ImeReply {
    NotQueried,
    Failed(u32),
    Raw(usize),
}
