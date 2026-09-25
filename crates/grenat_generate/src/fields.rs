//! The fields of a record, as written on the command line: `subject:String`,
//! `score:Float?` (optional).

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldType {
    String,
    Int,
    Float,
    Bool,
}

impl Field {
    pub fn parse(spec: &str) -> Result<Field, String> {
        let usage = || format!("invalid field `{spec}`: write `name:Type`, Type being String, Int, Float or Bool (`Int?`: optional)");
        let (name, ty) = spec.split_once(':').ok_or_else(usage)?;
        if !crate::names::valid(name) {
            return Err(usage());
        }
        if name == "id" {
            return Err("`id` is every record's own: the database gives it".into());
        }
        let (ty, optional) = ty.strip_suffix('?').map_or((ty, false), |t| (t, true));
        let ty = match ty {
            "String" => FieldType::String,
            "Int" => FieldType::Int,
            "Float" => FieldType::Float,
            "Bool" => FieldType::Bool,
            _ => return Err(usage()),
        };
        Ok(Field { name: name.to_string(), ty, optional })
    }

    /// Its declaration in a `struct`.
    pub fn declaration(&self) -> String {
        format!("{}: {}{}", self.name, self.type_name(), if self.optional { "?" } else { "" })
    }

    /// Its column: SQL that SQLite and PostgreSQL both accept.
    pub fn column(&self) -> String {
        let sql = match self.ty {
            FieldType::String => "TEXT",
            FieldType::Int => "BIGINT",
            FieldType::Float => "DOUBLE PRECISION",
            FieldType::Bool => "BOOLEAN",
        };
        format!("{} {sql}{}", self.name, if self.optional { "" } else { " NOT NULL" })
    }

    /// A value for tests.
    pub fn sample(&self) -> String {
        match self.ty {
            FieldType::String => format!("\"{}\"", self.name),
            FieldType::Int => "1".into(),
            FieldType::Float => "1.5".into(),
            FieldType::Bool => "true".into(),
        }
    }

    fn type_name(&self) -> &'static str {
        match self.ty {
            FieldType::String => "String",
            FieldType::Int => "Int",
            FieldType::Float => "Float",
            FieldType::Bool => "Bool",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields() {
        let score = Field::parse("score:Float?").unwrap();
        assert_eq!(score.declaration(), "score: Float?");
        assert_eq!(score.column(), "score DOUBLE PRECISION");
        let subject = Field::parse("subject:String").unwrap();
        assert_eq!(subject.column(), "subject TEXT NOT NULL");
        assert_eq!(subject.sample(), "\"subject\"");
        assert!(Field::parse("id:Int").unwrap_err().contains("database gives it"));
        for bad in ["subject", "subject:Text", "Subject:String", ":Int"] {
            assert!(Field::parse(bad).is_err(), "{bad}");
        }
    }
}
