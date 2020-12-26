mod layouts;
mod lease;

pub use self::lease::Lease;

use {
    self::layouts::Layouts,
    super::{
        def::{
            render_passes::{color, draw, draw_post, draw_pre, draw_pre_post},
            Compute, ComputeMode, Graphics, GraphicsMode, RenderPassMode,
        },
        driver::{CommandPool, DescriptorPool, Driver, Fence, Image2d, Memory, RenderPass},
        op::Compiler,
        BlendMode, Data, Texture, TextureRef,
    },
    crate::{math::Extent, pak::IndexType},
    gfx_hal::{
        buffer::Usage as BufferUsage,
        format::Format,
        image::{Layout, Usage as ImageUsage},
        pool::CommandPool as _,
        pso::{DescriptorRangeDesc, DescriptorType},
        queue::QueueFamilyId,
        MemoryTypeId,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        rc::Rc,
    },
};

#[cfg(feature = "debug-names")]
use gfx_hal::device::Device as _;

const DEFAULT_LRU_THRESHOLD: usize = 8;

fn remove_last_by<T, F: Fn(&T) -> bool>(items: &mut VecDeque<T>, f: F) -> Option<T> {
    // let len = items.len();
    // TODO: This is no longer remove by last!!
    for idx in 0..items.len() {
        if f(&items[idx]) {
            return Some(items.remove(idx).unwrap());
        }
    }

    None
}

pub(super) type PoolRef<T> = Rc<RefCell<VecDeque<T>>>;

#[derive(Eq, Hash, PartialEq)]
struct DescriptorPoolKey {
    desc_ranges: Vec<(DescriptorType, usize)>,
}

pub struct Drain<'a>(&'a mut Pool);

impl<'a> Iterator for Drain<'a> {
    type Item = ();

    fn next(&mut self) -> Option<()> {
        unimplemented!();
    }
}

#[derive(Eq, Hash, PartialEq)]
struct GraphicsKey {
    graphics_mode: GraphicsMode,
    render_pass_mode: RenderPassMode,
    subpass_idx: u8,
}

pub struct Pool {
    cmd_pools: HashMap<QueueFamilyId, PoolRef<CommandPool>>,
    compilers: PoolRef<Compiler>,
    computes: HashMap<ComputeMode, PoolRef<Compute>>,
    data: HashMap<BufferUsage, PoolRef<Data>>,
    desc_pools: HashMap<DescriptorPoolKey, PoolRef<DescriptorPool>>,
    fences: PoolRef<Fence>,
    graphics: HashMap<GraphicsKey, PoolRef<Graphics>>,
    pub(super) layouts: Layouts,

    /// The number of frames which must elapse before a least-recently-used cache item is considered obsolete.
    ///
    /// Remarks: Higher numbers such as 10 will use more memory but have less thrashing than lower numbers, such as 1.
    pub lru_threshold: usize,

    memories: HashMap<MemoryTypeId, PoolRef<Memory>>,
    render_passes: HashMap<RenderPassMode, RenderPass>,
    textures: HashMap<TextureKey, PoolRef<TextureRef<Image2d>>>,
}

// TODO: Add some way to track memory usage so that using drain has some sort of feedback for users, tell them about the usage
impl Pool {
    pub(super) fn cmd_pool(
        &mut self,
        driver: &Driver,
        family: QueueFamilyId,
    ) -> Lease<CommandPool> {
        let items = self
            .cmd_pools
            .entry(family)
            .or_insert_with(Default::default);
        let mut item = if let Some(item) = items.borrow_mut().pop_back() {
            item
        } else {
            CommandPool::new(driver, family)
        };

        unsafe {
            item.as_mut().reset(false);
        }

        Lease::new(item, items)
    }

    pub(super) fn compiler(&mut self) -> Lease<Compiler> {
        let item = if let Some(item) = self.compilers.borrow_mut().pop_back() {
            item
        } else {
            debug!("Creating new compiler");
            Default::default()
        };

        Lease::new(item, &self.compilers)
    }

    pub(super) fn compute(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        mode: ComputeMode,
    ) -> Lease<Compute> {
        self.compute_desc_sets(
            #[cfg(feature = "debug-names")]
            name,
            driver,
            mode,
            1,
        )
    }

    pub(super) fn compute_desc_sets(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        mode: ComputeMode,
        max_desc_sets: usize,
    ) -> Lease<Compute> {
        let items = self.computes.entry(mode).or_insert_with(Default::default);
        let item = if let Some(item) = remove_last_by(&mut items.borrow_mut(), |item| {
            item.max_desc_sets() >= max_desc_sets
        }) {
            item
        } else {
            let ctor = match mode {
                ComputeMode::CalcVertexAttrs(m) if m.idx_ty == IndexType::U16 && !m.skin => {
                    Compute::calc_vertex_attrs_u16
                }
                ComputeMode::CalcVertexAttrs(m) if m.idx_ty == IndexType::U16 && m.skin => {
                    Compute::calc_vertex_attrs_u16_skin
                }
                ComputeMode::CalcVertexAttrs(m) if m.idx_ty == IndexType::U32 && !m.skin => {
                    Compute::calc_vertex_attrs_u32
                }
                ComputeMode::CalcVertexAttrs(m) if m.idx_ty == IndexType::U32 && m.skin => {
                    Compute::calc_vertex_attrs_u32_skin
                }
                ComputeMode::DecodeRgbRgba => Compute::decode_rgb_rgba,
                _ => unreachable!(),
            };
            let (desc_set_layout, pipeline_layout) = match mode {
                ComputeMode::CalcVertexAttrs(_) => self.layouts.compute_calc_vertex_attrs(
                    #[cfg(feature = "debug-names")]
                    name,
                    driver,
                ),
                ComputeMode::DecodeRgbRgba => self.layouts.compute_decode_rgb_rgba(
                    #[cfg(feature = "debug-names")]
                    name,
                    driver,
                ),
            };

            unsafe {
                ctor(
                    #[cfg(feature = "debug-names")]
                    name,
                    driver,
                    desc_set_layout,
                    pipeline_layout,
                    max_desc_sets,
                )
            }
        };

        Lease::new(item, items)
    }

    pub(super) fn data(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        len: u64,
    ) -> Lease<Data> {
        self.data_usage(
            #[cfg(feature = "debug-names")]
            name,
            driver,
            len,
            BufferUsage::empty(),
        )
    }

    pub(super) fn data_usage(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        len: u64,
        usage: BufferUsage,
    ) -> Lease<Data> {
        let items = self.data.entry(usage).or_insert_with(Default::default);
        let item = if let Some(item) =
            remove_last_by(&mut items.borrow_mut(), |item| item.capacity() >= len)
        {
            item
        } else {
            Data::new(
                #[cfg(feature = "debug-names")]
                name,
                driver,
                len,
                usage,
            )
        };

        Lease::new(item, items)
    }

    // TODO: I don't really like the function signature here
    pub(super) fn desc_pool<'i, I>(
        &mut self,
        driver: &Driver,
        max_desc_sets: usize,
        desc_ranges: I,
    ) -> Lease<DescriptorPool>
    where
        I: Clone + ExactSizeIterator<Item = &'i DescriptorRangeDesc>,
    {
        let desc_ranges_key = desc_ranges
            .clone()
            .map(|desc_range| (desc_range.ty, desc_range.count))
            .collect();
        // TODO: Sort (and possibly combine) desc_ranges so that different orders of the same data don't affect key lookups
        let items = self
            .desc_pools
            .entry(DescriptorPoolKey {
                desc_ranges: desc_ranges_key,
            })
            .or_insert_with(Default::default);
        let item = if let Some(item) = remove_last_by(&mut items.borrow_mut(), |item| {
            DescriptorPool::max_desc_sets(&item) >= max_desc_sets
        }) {
            item
        } else {
            DescriptorPool::new(driver, max_desc_sets, desc_ranges)
        };

        Lease::new(item, items)
    }

    /// Allows callers to remove unused memory-consuming items from the pool.
    pub fn drain(&mut self) -> Drain {
        Drain(self)
    }

    pub(super) fn fence(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
    ) -> Lease<Fence> {
        let item = if let Some(mut item) = self.fences.borrow_mut().pop_back() {
            Fence::reset(&mut item);
            item
        } else {
            Fence::new(
                #[cfg(feature = "debug-names")]
                name,
                driver,
            )
        };

        Lease::new(item, &self.fences)
    }

    pub(super) fn graphics(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        graphics_mode: GraphicsMode,
        render_pass_mode: RenderPassMode,
        subpass_idx: u8,
    ) -> Lease<Graphics> {
        self.graphics_desc_sets(
            #[cfg(feature = "debug-names")]
            name,
            driver,
            graphics_mode,
            render_pass_mode,
            subpass_idx,
            1,
        )
    }

    pub(super) fn graphics_desc_sets(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        graphics_mode: GraphicsMode,
        render_pass_mode: RenderPassMode,
        subpass_idx: u8,
        max_desc_sets: usize,
    ) -> Lease<Graphics> {
        {
            let items = self
                .graphics
                .entry(GraphicsKey {
                    graphics_mode,
                    render_pass_mode,
                    subpass_idx,
                })
                .or_insert_with(Default::default);
            if let Some(item) = remove_last_by(&mut items.borrow_mut(), |item| {
                item.max_desc_sets() >= max_desc_sets
            }) {
                return Lease::new(item, items);
            }
        }
        let ctor = match graphics_mode {
            GraphicsMode::Blend(BlendMode::Normal) => Graphics::blend_normal,
            GraphicsMode::Blend(_) => todo!(),
            GraphicsMode::DrawLine => Graphics::draw_line,
            GraphicsMode::DrawMesh => Graphics::draw_mesh,
            GraphicsMode::DrawPointLight => Graphics::draw_point_light,
            GraphicsMode::DrawRectLight => Graphics::draw_rect_light,
            GraphicsMode::DrawSpotlight => Graphics::draw_spotlight,
            GraphicsMode::DrawSunlight => Graphics::draw_sunlight,
            GraphicsMode::Font => Graphics::font,
            GraphicsMode::FontOutline => Graphics::font_outline,
            GraphicsMode::Gradient => Graphics::gradient,
            GraphicsMode::GradientTransparency => Graphics::gradient_transparency,
            GraphicsMode::Texture => Graphics::texture,
        };
        let item = unsafe {
            ctor(
                #[cfg(feature = "debug-names")]
                name,
                driver,
                max_desc_sets,
                RenderPass::subpass(self.render_pass(driver, render_pass_mode), subpass_idx),
            )
        };

        let items = &self.graphics[&GraphicsKey {
            graphics_mode,
            render_pass_mode,
            subpass_idx,
        }];
        Lease::new(item, items)
    }

    pub(super) fn memory(
        &mut self,
        driver: &Driver,
        mem_type: MemoryTypeId,
        size: u64,
    ) -> Lease<Memory> {
        let items = self
            .memories
            .entry(mem_type)
            .or_insert_with(Default::default);
        let item = if let Some(item) =
            remove_last_by(&mut items.borrow_mut(), |item| Memory::size(&item) >= size)
        {
            item
        } else {
            Memory::new(driver, mem_type, size)
        };

        Lease::new(item, items)
    }

    pub(super) fn render_pass(&mut self, driver: &Driver, mode: RenderPassMode) -> &RenderPass {
        self.render_passes
            .entry(mode)
            .or_insert_with(|| match mode {
                RenderPassMode::Color(mode) => color(driver, mode),
                RenderPassMode::Draw(mode) => {
                    if mode.pre_fx as u8 * mode.post_fx as u8 == 1 {
                        draw_pre_post(driver, mode)
                    } else if mode.pre_fx {
                        draw_pre(driver, mode)
                    } else if mode.post_fx {
                        draw_post(driver, mode)
                    } else {
                        draw(driver, mode)
                    }
                }
            })
    }

    // TODO: Bubble format picking up and out of this! (removes desire_tiling+desired_fmts+features, replace with fmt/tiling)
    #[allow(clippy::too_many_arguments)]
    pub(super) fn texture(
        &mut self,
        #[cfg(feature = "debug-names")] name: &str,
        driver: &Driver,
        dims: Extent,
        fmt: Format,
        layout: Layout,
        usage: ImageUsage,
        layers: u16,
        mips: u8,
        samples: u8,
    ) -> Lease<TextureRef<Image2d>> {
        let items = self
            .textures
            .entry(TextureKey {
                dims,
                fmt,
                layers,
                mips,
                samples,
                usage,
            })
            .or_insert_with(Default::default);
        let item = {
            let mut items_ref = items.as_ref().borrow_mut();
            if let Some(item) = items_ref.pop_back() {
                // Set a new name on this texture
                #[cfg(feature = "debug-names")]
                unsafe {
                    driver
                        .as_ref()
                        .borrow()
                        .set_image_name(item.as_ref().borrow_mut().as_mut(), name);
                }

                item
            } else {
                // Add a cache item so there will be an unused item waiting next time
                items_ref.push_front(TextureRef::new(RefCell::new(Texture::new(
                    #[cfg(feature = "debug-names")]
                    &format!("{} (Unused)", name),
                    driver,
                    dims,
                    fmt,
                    layout,
                    usage,
                    layers,
                    samples,
                    mips,
                ))));

                // Return a brand new instance
                TextureRef::new(RefCell::new(Texture::new(
                    #[cfg(feature = "debug-names")]
                    name,
                    driver,
                    dims,
                    fmt,
                    layout,
                    usage,
                    layers,
                    samples,
                    mips,
                )))
            }
        };

        Lease::new(item, items)
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            cmd_pools: Default::default(),
            compilers: Default::default(),
            computes: Default::default(),
            data: Default::default(),
            desc_pools: Default::default(),
            fences: Default::default(),
            graphics: Default::default(),
            layouts: Default::default(),
            lru_threshold: DEFAULT_LRU_THRESHOLD,
            memories: Default::default(),
            render_passes: Default::default(),
            textures: Default::default(),
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // Make sure these get dropped before the layouts! (They contain unsafe references!)
        self.computes.clear();
        self.graphics.clear();
    }
}

#[derive(Eq, Hash, PartialEq)]
struct TextureKey {
    dims: Extent,
    fmt: Format,
    layers: u16,
    mips: u8,
    samples: u8,
    usage: ImageUsage, // TODO: Usage shouldn't be a hard filter like this
}
