use crate::compress::quadtree::stats::{Stats, StatsReporting};
use crate::compress::Mapping;
use crate::image::block::{IntoSquaredBlocks, SquaredBlock};
use crate::image::downscale::IntoDownscaled;
use crate::image::Image;
use crate::model::{Block, Compressed, Transformation};
use itertools::Itertools;
use std::collections::VecDeque;
use std::rc::Rc;
use tracing::{debug, info, instrument};

pub struct Compressor<I> {
    image: I,
    error_threshold: ErrorThreshold,
    progress_fn: Option<Rc<dyn Fn(stats::StatsReporting)>>,
}

impl<I> Compressor<I>
where
    I: Image,
{
    pub fn new(image: I) -> Self {
        Self {
            image,
            error_threshold: ErrorThreshold::default(),
            progress_fn: None,
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub fn compress(self) -> Compressed {
        let image_height = self.image.get_height();
        let image_width = self.image.get_width();
        assert_eq!(
            image_height, image_width,
            "Only square sized images supported"
        );
        info!("Compressing {}x{} image", image_height, image_width);
        let image = Rc::new(self.image);

        let mut transformations: Vec<Transformation> = Vec::new();

        let domain_block_size: u32 = image.get_height();
        let range_block_size: u32 = (image.get_height() as f64 / 2.0) as u32;

        let domain_blocks = image.squared_blocks(domain_block_size);
        let range_blocks = image
            .squared_blocks(range_block_size)
            .into_iter()
            .map(Rc::new)
            .collect::<Vec<Rc<_>>>();

        debug!(
            "Domain blocks: {} with size {}x{}",
            domain_blocks.len(),
            domain_block_size,
            domain_block_size
        );
        debug!(
            "Range blocks: {} with size {}x{}",
            range_blocks.len(),
            range_block_size,
            range_block_size
        );

        let mut stats = Stats::new(image.get_height());

        let mut queue = VecDeque::from(range_blocks);
        while let Some(rb) = queue.pop_front() {
            debug!(
                "Finding transformation for range block {} (remaining: {})",
                rb,
                queue.len()
            );
            match Transformation::find(image.clone(), &rb, self.error_threshold) {
                Some(transformation) => {
                    debug!("For range block {}, found best matching domain block", rb);
                    stats.report_block_mapped(rb.get_height()); // TODO: Only when we have progress!
                    if let Some(progress_fn) = self.progress_fn.clone() {
                        progress_fn(stats.report())
                    }
                    transformations.push(transformation)
                }
                None => {
                    debug!("For range block {}, found no matching domain block", rb);
                    if rb.get_height() <= 1 {
                        panic!("Nope"); // TODO
                    } else {
                        let new_range_blocks =
                            rb.squared_blocks((rb.get_height() as f64 / 2.0) as u32);
                        let new_range_blocks =
                            new_range_blocks.into_iter().map(|nrb| nrb.flatten());
                        new_range_blocks
                            .into_iter()
                            .for_each(|nrb| queue.push_back(Rc::new(nrb)))
                    }
                }
            }
        }

        transformations.into()
    }

    pub fn with_error_threshold(mut self, error_threshold: ErrorThreshold) -> Self {
        self.error_threshold = error_threshold;
        self
    }

    pub fn with_progress_reporter<F: Fn(StatsReporting) + 'static>(
        mut self,
        progress_fn: F,
    ) -> Self {
        self.progress_fn = Some(Rc::new(progress_fn));
        self
    }
}

struct DomainBlockMapping<T>((T, Mapping));

impl<T> PartialEq for DomainBlockMapping<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 .1.error == other.0 .1.error
    }
}

impl<T> Eq for DomainBlockMapping<T> {}

impl<T> PartialOrd for DomainBlockMapping<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.0 .1.error < other.0 .1.error {
            true => Some(std::cmp::Ordering::Less),
            false => Some(std::cmp::Ordering::Greater),
        }
    }
}

impl<T> Ord for DomainBlockMapping<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.0 .1.error < other.0 .1.error {
            true => std::cmp::Ordering::Less,
            false => std::cmp::Ordering::Greater,
        }
    }
}

impl Transformation {
    fn find<I: Image>(
        image: Rc<I>,
        range_block: &SquaredBlock<I>,
        error_threshold: ErrorThreshold,
    ) -> Option<Self> {
        let range_block_size = range_block.size;
        let domain_block_size = 2 * range_block_size;

        let domain_blocks = image.squared_blocks(domain_block_size);

        let mapping = domain_blocks
            .iter()
            .map(|d| d.downscale_2x2())
            .map(|db| {
                let mapping = Mapping::compute(&db, range_block);
                debug!("Mapping: {:?}", mapping);
                (db, mapping)
            })
            .filter(|(_, mapping)| match error_threshold {
                ErrorThreshold::RmsAnyLowerThan(acceptable_error) => {
                    mapping.error < acceptable_error
                }
            })
            .map(|x| DomainBlockMapping(x))
            .sorted()
            .map(|x| x.0)
            .take(1)
            .next();

        if let Some((db, mapping)) = mapping {
            debug!("Using mapping: {:?}", mapping);
            return Some(Self {
                range: Block {
                    block_size: range_block.size,
                    image_size: image.get_height(),
                    origin: range_block.origin,
                },
                domain: Block {
                    block_size: db.inner().size,
                    image_size: image.get_height(),
                    origin: db.inner().origin,
                },
                brightness: mapping.brightness,
                saturation: mapping.saturation,
            });
        }

        None
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ErrorThreshold {
    RmsAnyLowerThan(f64),
}

impl Default for ErrorThreshold {
    fn default() -> Self {
        Self::RmsAnyLowerThan(5.0)
    }
}

mod stats {

    #[derive(Clone, Copy, Debug)]
    pub struct StatsReporting {
        pub area_covered: u32,
        pub total_area: u32,
    }

    impl StatsReporting {
        pub fn finished(&self) -> bool {
            self.area_covered == self.total_area
        }
    }

    /// Records the area of the image that has already been mapped
    pub struct Stats {
        pub image_size_squared: u32,
        pub area_covered: u32,
    }

    impl Stats {
        pub fn new(image_size: u32) -> Self {
            Self {
                image_size_squared: image_size * image_size,
                area_covered: 0,
            }
        }

        pub fn report_block_mapped(&mut self, range_block_size: u32) {
            self.area_covered += range_block_size * range_block_size
        }

        pub fn report(&self) -> StatsReporting {
            StatsReporting {
                area_covered: self.area_covered,
                total_area: self.image_size_squared,
            }
        }
    }
}
